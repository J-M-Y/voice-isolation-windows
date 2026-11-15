// Pure adapter-selection regression tests: no model, GPU or audio required.
// Compile alongside the DirectML worker's normal Windows/DXGI toolchain.
#define VOICE_GPU_SELECTION_TEST
#include "worker.cpp"
#include <cassert>
#include <iostream>

// The unused model paths in worker.cpp must not need a loaded runtime for
// this isolated selection test.
extern "C" {
void *vox_v1_create(const wchar_t *, const wchar_t *, const wchar_t *, const float *, size_t, int, int, char *, size_t) { return nullptr; }
void *vox_v1_create_fixed(const wchar_t *, const wchar_t *, const wchar_t *, const float *, size_t, int, int, size_t, char *, size_t) { return nullptr; }
int vox_v1_push(void *, const float *, size_t, float *, size_t, size_t *, char *, size_t) { return -1; }
int vox_v1_finish_profile(void *, char *, size_t, char *, size_t) { return -1; }
void vox_v1_destroy(void *) {}
}

int main() {
    assert(is_hardware_adapter(0));
    assert(is_hardware_adapter(DXGI_ADAPTER_FLAG_REMOTE));
    assert(!is_hardware_adapter(DXGI_ADAPTER_FLAG_SOFTWARE));
    assert(!is_hardware_adapter(DXGI_ADAPTER_FLAG_SOFTWARE | DXGI_ADAPTER_FLAG_REMOTE));

    // Original DXGI indices are deliberately nonconsecutive, as they would
    // be when software devices were filtered out. Same names are not identity.
    const std::vector<Adapter> adapters = {
        {0, 0x1001, 0x1002, 1, 4, "Same name"}, // AMD
        {3, 0x1002, 0x10de, 2, 8, "Same name"}, // NVIDIA
        {7, 0x1003, 0x8086, 3, 0, "Intel"},
        {9, 0x1004, 0xabcd, 4, 1, "Unknown hardware"},
        {12, 0x1005, 0x10de, 5, 8, "Other NVIDIA"},
    };
    int selected = -1; std::string name; char error[256] = {};
    for (const auto &a : adapters) {
        assert(choose_hardware(adapters, a.index, a.luid, &selected, &name, error, sizeof(error)));
        assert(selected == static_cast<int>(a.index));
        assert(name == a.name);
    }
    assert(choose_hardware(adapters, -1, 0, &selected, &name, error, sizeof(error)));
    assert(selected == 3); // Largest dedicated memory; first on ties.
    assert(!choose_hardware(adapters, 3, 0x1001, &selected, &name, error, sizeof(error)));
    assert(!choose_hardware(adapters, 1, 0x1001, &selected, &name, error, sizeof(error)));
    assert(!choose_hardware({}, -1, 0, &selected, &name, error, sizeof(error)));
    std::cout << "adapter selection tests passed\n";
}
