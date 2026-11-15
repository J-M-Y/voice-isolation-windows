// Experimental GPU worker. A dedicated process keeps its DirectML ONNX Runtime
// separate from the CPU runtime used by the main application's denoiser.
// Binary protocol: u32 magic, reference (48,000 f32), status and device identity;
// then repeated u32 length,
// f32 input -> u32 length, f32 output. Length 0 shuts down. 0xffffffff denotes
// an error followed by u32 UTF-8 byte length and that many bytes. No raw input
// is ever emitted on failure. This worker is not yet wired into the UI.
#include <windows.h>
#include <dxgi1_2.h>
#include <algorithm>
#include <fcntl.h>
#include <io.h>
#include <cstdint>
#include <cctype>
#include <cmath>
#include <cstdio>
#include <cstdlib>
#include <cstring>
#include <cwchar>
#include <cwctype>
#include <memory>
#include <string>
#include <vector>

extern "C" {
void *vox_v1_create(const wchar_t *, const wchar_t *, const wchar_t *, const float *, size_t,
                    int, int, char *, size_t);
void *vox_v1_create_fixed(const wchar_t *, const wchar_t *, const wchar_t *, const float *, size_t,
                          int, int, size_t, char *, size_t);
int vox_v1_push(void *, const float *, size_t, float *, size_t, size_t *, char *, size_t);
int vox_v1_finish_profile(void *, char *, size_t, char *, size_t);
void vox_v1_destroy(void *);
}

namespace {
constexpr uint32_t kError = 0xffffffffu;
constexpr uint32_t kProtocolMagic = 0x32555047u; // ASCII GPU2 in little endian
constexpr uint32_t kReferenceCount = 48000;
constexpr uint32_t kMaxBlock = 16000;

bool read_exact(void *dst, size_t bytes) {
    auto *p = static_cast<unsigned char *>(dst);
    while (bytes) {
        size_t n = std::fread(p, 1, bytes, stdin);
        if (!n) return false;
        p += n; bytes -= n;
    }
    return true;
}
bool write_exact(const void *src, size_t bytes) {
    const auto *p = static_cast<const unsigned char *>(src);
    while (bytes) {
        size_t n = std::fwrite(p, 1, bytes, stdout);
        if (!n) return false;
        p += n; bytes -= n;
    }
    return true;
}
void send_error(const char *message) {
    uint32_t size = static_cast<uint32_t>(std::min<size_t>(std::strlen(message), 4096));
    write_exact(&kError, sizeof(kError));
    write_exact(&size, sizeof(size));
    write_exact(message, size);
    std::fflush(stdout);
}
struct Destroy { void operator()(void *p) const { if (p) vox_v1_destroy(p); } };

struct Adapter {
    UINT index;
    uint64_t luid;
    UINT vendor_id, device_id;
    SIZE_T dedicated_memory;
    std::string name;
};
bool is_hardware_adapter(UINT flags) { return (flags & DXGI_ADAPTER_FLAG_SOFTWARE) == 0; }

bool enumerate_hardware(std::vector<Adapter> *out, char *error, size_t error_size) {
    IDXGIFactory1 *factory = nullptr;
    HRESULT hr = CreateDXGIFactory1(IID_IDXGIFactory1, reinterpret_cast<void **>(&factory));
    if (FAILED(hr) || !factory) { std::snprintf(error, error_size, "cannot enumerate DXGI adapters"); return false; }
    for (UINT i = 0;; ++i) {
        IDXGIAdapter1 *adapter = nullptr;
        hr = factory->EnumAdapters1(i, &adapter);
        if (hr == DXGI_ERROR_NOT_FOUND) break;
        if (FAILED(hr) || !adapter) { factory->Release(); std::snprintf(error, error_size, "DXGI adapter enumeration failed at %u", i); return false; }
        DXGI_ADAPTER_DESC1 desc{};
        hr = adapter->GetDesc1(&desc);
        adapter->Release();
        if (FAILED(hr)) { factory->Release(); std::snprintf(error, error_size, "cannot inspect DXGI adapter %u", i); return false; }
        if (!is_hardware_adapter(desc.Flags)) continue;
        if (out->size() >= 128) { factory->Release(); std::snprintf(error, error_size, "too many hardware adapters"); return false; }
        int n = WideCharToMultiByte(CP_UTF8, 0, desc.Description, -1, nullptr, 0, nullptr, nullptr);
        if (n <= 1 || n > 1024) continue;
        std::vector<char> bytes(static_cast<size_t>(n));
        if (!WideCharToMultiByte(CP_UTF8, 0, desc.Description, -1, bytes.data(), n, nullptr, nullptr)) continue;
        const uint64_t luid = (uint64_t(static_cast<uint32_t>(desc.AdapterLuid.HighPart)) << 32)
                            | static_cast<uint32_t>(desc.AdapterLuid.LowPart);
        out->push_back(Adapter{i, luid, desc.VendorId, desc.DeviceId, desc.DedicatedVideoMemory, bytes.data()});
    }
    factory->Release();
    return true;
}

bool choose_hardware(const std::vector<Adapter> &adapters, long requested, uint64_t expected_luid, int *selected, std::string *name, char *error, size_t error_size) {
    if (adapters.empty()) { std::snprintf(error, error_size, "no hardware DXGI adapter found"); return false; }
    const Adapter *chosen = nullptr;
    if (requested >= 0) {
        for (const auto &adapter : adapters) if (adapter.index == static_cast<UINT>(requested)) { chosen = &adapter; break; }
        if (!chosen) { std::snprintf(error, error_size, "selected hardware GPU is no longer available; refresh devices"); return false; }
        if (chosen->luid != expected_luid) { std::snprintf(error, error_size, "GPU selection changed; refresh devices and select again"); return false; }
    } else {
        // Prefer dedicated memory on multi-GPU systems without excluding
        // integrated hardware. Initialization still verifies real DML nodes.
        chosen = &*std::max_element(adapters.begin(), adapters.end(), [](const Adapter &a, const Adapter &b) {
            return a.dedicated_memory < b.dedicated_memory;
        });
    }
    *selected = static_cast<int>(chosen->index);
    *name = chosen->name;
    return true;
}

bool select_hardware(long requested, uint64_t expected_luid, int *selected, std::string *name, char *error, size_t error_size) {
    std::vector<Adapter> adapters;
    if (!enumerate_hardware(&adapters, error, error_size)) return false;
    return choose_hardware(adapters, requested, expected_luid, selected, name, error, error_size);
}

int print_adapters() {
    std::vector<Adapter> adapters; char error[2048] = {};
    if (!enumerate_hardware(&adapters, error, sizeof(error))) { std::fprintf(stderr, "%s\n", error); return 1; }
    if (std::printf("GPU2-LIST\t1\n") < 0) return 1;
    static const char digits[] = "0123456789ABCDEF";
    for (const auto &a : adapters) {
        if (std::printf("%u\t%016llX\t%08X\t%08X\t", a.index,
                        static_cast<unsigned long long>(a.luid), a.vendor_id, a.device_id) < 0) return 1;
        for (unsigned char byte : a.name) {
            if (std::putchar(digits[byte >> 4]) == EOF || std::putchar(digits[byte & 15]) == EOF) return 1;
        }
        if (std::putchar('\n') == EOF) return 1;
    }
    return std::fflush(stdout) == 0 ? 0 : 1;
}

std::wstring wide_utf8(const char *s) {
    int n = MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, s, -1, nullptr, 0);
    if (n <= 0) return {};
    std::vector<wchar_t> result(static_cast<size_t>(n));
    if (!MultiByteToWideChar(CP_UTF8, MB_ERR_INVALID_CHARS, s, -1, result.data(), n)) return {};
    return std::wstring(result.data());
}
std::string json_string_field(const std::string &line, const std::string &key) {
    auto pos = line.find("\"" + key + "\"");
    if (pos == std::string::npos) return {};
    pos = line.find(':', pos + key.size() + 2);
    if (pos == std::string::npos) return {};
    pos = line.find_first_not_of(" \t\r\n", pos + 1);
    if (pos == std::string::npos || line[pos] != '"') return {};
    auto end = line.find('"', ++pos);
    if (end == std::string::npos) return {};
    return line.substr(pos, end - pos);
}
bool profile_has_dml_node(const std::wstring &path) {
    FILE *file = _wfopen(path.c_str(), L"rb");
    if (!file) return false;
    std::string text; char buffer[32768]; size_t n = 0;
    while ((n = std::fread(buffer, 1, sizeof(buffer), file)) != 0) {
        if (text.size() + n > 64 * 1024 * 1024) { std::fclose(file); return false; }
        text.append(buffer, n);
    }
    bool read_failed = std::ferror(file) != 0;
    std::fclose(file);
    if (read_failed) return false;
    for (size_t begin = 0; begin < text.size();) {
        size_t end = text.find('\n', begin);
        if (end == std::string::npos) end = text.size();
        std::string line = text.substr(begin, end - begin);
        begin = end + 1;
        if (json_string_field(line, "cat") != "Node" || json_string_field(line, "provider") != "DmlExecutionProvider") continue;
        auto op = json_string_field(line, "op_name");
        std::transform(op.begin(), op.end(), op.begin(), [](unsigned char c) { return static_cast<char>(std::tolower(c)); });
        // Host/device copies alone are not proof that model computation ran on GPU.
        if (!op.empty() && op.find("copy") == std::string::npos && op.find("transfer") == std::string::npos) return true;
    }
    return false;
}
bool verify_gpu(const wchar_t *runtime, const wchar_t *enroll, const wchar_t *chunk,
                int adapter, size_t block, char *error, size_t error_size) {
    wchar_t dir[32768];
    DWORD n = GetTempPathW(32768, dir);
    if (!n || n >= 32768) { std::snprintf(error, error_size, "cannot locate temporary directory"); return false; }
    std::wstring prefix = std::wstring(dir) + L"voice-gpu-probe-" + std::to_wstring(GetCurrentProcessId()) + L"-" + std::to_wstring(GetTickCount64());
    struct Cleanup {
        std::wstring prefix;
        ~Cleanup() {
            SetEnvironmentVariableW(L"VOICE_GPU_PROFILE", nullptr);
            // These are synthetic diagnostics made by this process only.
            WIN32_FIND_DATAW data{};
            HANDLE find = FindFirstFileW((prefix + L"-*.json").c_str(), &data);
            if (find == INVALID_HANDLE_VALUE) return;
            auto slash = prefix.find_last_of(L"\\/");
            const std::wstring directory = prefix.substr(0, slash + 1);
            do { if (!(data.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY)) DeleteFileW((directory + data.cFileName).c_str()); }
            while (FindNextFileW(find, &data));
            FindClose(find);
        }
    } cleanup{prefix};
    if (!SetEnvironmentVariableW(L"VOICE_GPU_PROFILE", prefix.c_str())) { std::snprintf(error, error_size, "cannot enable GPU verification"); return false; }
    // The provider probe uses generated data only, never the user's voice.
    std::vector<float> synthetic(kReferenceCount);
    for (size_t i = 0; i < synthetic.size(); ++i) synthetic[i] = 0.1f * std::sin(2.0 * 3.14159265358979323846 * 220.0 * static_cast<double>(i) / 16000.0);
    std::unique_ptr<void, Destroy> probe(vox_v1_create_fixed(runtime, enroll, chunk, synthetic.data(), synthetic.size(), 1, adapter, block, error, error_size));
    if (!probe) { SetEnvironmentVariableW(L"VOICE_GPU_PROFILE", nullptr); return false; }
    std::vector<float> input(block, 0.0f), output(block + 1024);
    for (int i = 0; i < 2; ++i) {
        size_t written = 0;
        if (vox_v1_push(probe.get(), input.data(), input.size(), output.data(), output.size(), &written, error, error_size)) {
            SetEnvironmentVariableW(L"VOICE_GPU_PROFILE", nullptr); return false;
        }
    }
    char profile_path[32768] = {};
    if (vox_v1_finish_profile(probe.get(), profile_path, sizeof(profile_path), error, error_size)) {
        SetEnvironmentVariableW(L"VOICE_GPU_PROFILE", nullptr); return false;
    }
    probe.reset();
    SetEnvironmentVariableW(L"VOICE_GPU_PROFILE", nullptr);
    std::string paths(profile_path); auto split = paths.find('\n');
    if (split == std::string::npos || paths.find('\n', split + 1) != std::string::npos) { std::snprintf(error, error_size, "invalid GPU profiling output paths"); return false; }
    auto first = wide_utf8(paths.substr(0, split).c_str());
    auto steady = wide_utf8(paths.substr(split + 1).c_str());
    if (first.empty() || steady.empty() || first.rfind(prefix + L"-first", 0) != 0 || steady.rfind(prefix + L"-steady", 0) != 0) { std::snprintf(error, error_size, "invalid GPU profiling output path"); return false; }
    bool first_used = profile_has_dml_node(first), steady_used = profile_has_dml_node(steady);
    DeleteFileW(first.c_str()); DeleteFileW(steady.c_str());
    if (!first_used || !steady_used) { std::snprintf(error, error_size, "DirectML executed no compute nodes in first or steady session; GPU mode refused"); return false; }
    return true;
}
}

#ifndef VOICE_GPU_SELECTION_TEST
int wmain(int argc, wchar_t **argv) {
    _setmode(_fileno(stdin), _O_BINARY);
    _setmode(_fileno(stdout), _O_BINARY);
    // Only this worker's synthetic self-test may enable profiler output.
    SetEnvironmentVariableW(L"VOICE_GPU_PROFILE", nullptr);
    if (argc == 2 && std::wcscmp(argv[1], L"--list-adapters") == 0) return print_adapters();
    if (!write_exact(&kProtocolMagic, sizeof(kProtocolMagic)) || std::fflush(stdout)) return 1;
    if (argc != 8) { send_error("usage: worker <ort.dll> <enroll.onnx> <chunk.onnx> <cpu|dml> <adapter-id|-1-auto> <block-size-16khz> <expected-luid|->"); return 2; }
    int provider = std::wcscmp(argv[4], L"dml") == 0 ? 1 : std::wcscmp(argv[4], L"cpu") == 0 ? 0 : -1;
    wchar_t *end = nullptr;
    long adapter = std::wcstol(argv[5], &end, 10);
    if (provider < 0 || !end || end == argv[5] || *end || adapter < -1 || adapter > 1024) { send_error("invalid provider or adapter id"); return 2; }
    end = nullptr;
    long block = std::wcstol(argv[6], &end, 10);
    if (!end || end == argv[6] || *end || (block != 1280 && block != 2560 && block != 3840)) { send_error("invalid block size"); return 2; }
    uint64_t expected_luid = 0;
    if (adapter == -1) {
        if (std::wcscmp(argv[7], L"-") != 0) { send_error("automatic selection must not specify an adapter LUID"); return 2; }
    } else {
        if (std::wcslen(argv[7]) != 16) { send_error("expected GPU LUID must contain 16 hexadecimal digits"); return 2; }
        for (const wchar_t *p = argv[7]; *p; ++p) {
            unsigned int digit = 0;
            if (*p >= L'0' && *p <= L'9') digit = static_cast<unsigned int>(*p - L'0');
            else if (*p >= L'a' && *p <= L'f') digit = static_cast<unsigned int>(*p - L'a' + 10);
            else if (*p >= L'A' && *p <= L'F') digit = static_cast<unsigned int>(*p - L'A' + 10);
            else { send_error("invalid GPU LUID"); return 2; }
            expected_luid = (expected_luid << 4) | digit;
        }
    }
    std::vector<float> reference(kReferenceCount);
    if (!read_exact(reference.data(), reference.size() * sizeof(float))) return 1;
    char error[2048] = {};
    std::string device_name = "CPU";
    int selected_adapter = -1;
    if (provider == 1 && !select_hardware(adapter, expected_luid, &selected_adapter, &device_name, error, sizeof(error))) {
        send_error(error); return 1;
    }
    if (provider == 0) selected_adapter = 0;
    if (provider == 1 && !verify_gpu(argv[1], argv[2], argv[3], selected_adapter, static_cast<size_t>(block), error, sizeof(error))) {
        send_error(error[0] ? error : "GPU verification failed"); return 1;
    }
    std::unique_ptr<void, Destroy> stream(vox_v1_create_fixed(argv[1], argv[2], argv[3], reference.data(), reference.size(), provider, selected_adapter, static_cast<size_t>(block), error, sizeof(error)));
    // Do not retain the enrollment buffer any longer than needed.
    std::fill(reference.begin(), reference.end(), 0.0f);
    if (!stream) { send_error(error[0] ? error : "GPU model initialization failed"); return 1; }
    uint32_t ok = 0;
    uint32_t device_index = static_cast<uint32_t>(selected_adapter);
    uint32_t device_bytes = static_cast<uint32_t>(device_name.size());
    if (!write_exact(&ok, sizeof(ok)) || !write_exact(&device_index, sizeof(device_index)) || !write_exact(&device_bytes, sizeof(device_bytes)) || !write_exact(device_name.data(), device_bytes) || std::fflush(stdout)) return 1;
    for (;;) {
        uint32_t count = 0;
        if (!read_exact(&count, sizeof(count))) return 0;
        if (count == 0) return 0;
        if (count > kMaxBlock || count != static_cast<uint32_t>(block)) { send_error("input block does not match configured size"); return 1; }
        std::vector<float> input(count), output(count + 1024);
        if (!read_exact(input.data(), input.size() * sizeof(float))) return 1;
        size_t written = 0;
        error[0] = 0;
        if (vox_v1_push(stream.get(), input.data(), input.size(), output.data(), output.size(), &written, error, sizeof(error))) {
            send_error(error[0] ? error : "GPU model processing failed"); return 1;
        }
        if (written > output.size() || written >= kError) { send_error("invalid output length"); return 1; }
        uint32_t n = static_cast<uint32_t>(written);
        if (!write_exact(&n, sizeof(n)) || !write_exact(output.data(), n * sizeof(float)) || std::fflush(stdout)) return 1;
    }
}
#endif
