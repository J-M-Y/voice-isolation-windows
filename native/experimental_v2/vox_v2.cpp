// Experimental VoxLock V2 stream. No microphone or unprocessed pass-through.
// Build against ONNX Runtime C API v24 headers; dynamically loads the runtime.
#include <windows.h>
#include <onnxruntime_c_api.h>
#include <algorithm>
#include <array>
#include <cmath>
#include <cstring>
#include <limits>
#include <memory>
#include <stdexcept>
#include <string>
#include <vector>

namespace {
struct Stream {
    HMODULE library = nullptr;
    const OrtApi *api = nullptr;
    OrtEnv *env = nullptr;
    OrtSessionOptions *options = nullptr;
    OrtSession *enroll = nullptr, *chunk = nullptr;
    OrtMemoryInfo *memory = nullptr;
    std::array<OrtValue *,3> voice{};
    OrtValue *h = nullptr, *c = nullptr;
    std::vector<float> state, head, buf, ola, norm;
    std::array<float,512> window{};
    bool started = false;
    int drop = 0;

    ~Stream() {
        if (api) {
            for (auto *v : voice) if (v) api->ReleaseValue(v);
            if (h) api->ReleaseValue(h);
            if (c) api->ReleaseValue(c);
            if (memory) api->ReleaseMemoryInfo(memory);
            if (chunk) api->ReleaseSession(chunk);
            if (enroll) api->ReleaseSession(enroll);
            if (options) api->ReleaseSessionOptions(options);
            if (env) api->ReleaseEnv(env);
        }
        if (library) FreeLibrary(library);
    }
    void check(OrtStatus *status) {
        if (!status) return;
        std::string error = api->GetErrorMessage(status);
        api->ReleaseStatus(status);
        throw std::runtime_error(error);
    }
    void init(const wchar_t *runtime, const wchar_t *enroll_path, const wchar_t *chunk_path,
              const float *reference, size_t reference_count) {
        if (!runtime || !enroll_path || !chunk_path || !reference || reference_count != 48000)
            throw std::runtime_error("expected exactly 3 seconds of 16 kHz reference audio");
        for (size_t i=0;i<reference_count;i++) if (!std::isfinite(reference[i]))
            throw std::runtime_error("reference contains non-finite samples");
        library = LoadLibraryExW(runtime, nullptr, LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS);
        if (!library) throw std::runtime_error("cannot load ONNX Runtime DLL");
        auto get_base = reinterpret_cast<const OrtApiBase *(ORT_API_CALL *)()>(GetProcAddress(library,"OrtGetApiBase"));
        if (!get_base || !(api = get_base()->GetApi(ORT_API_VERSION)))
            throw std::runtime_error("ONNX Runtime C API version unavailable");
        check(api->CreateEnv(ORT_LOGGING_LEVEL_WARNING,"vox-v2-experiment",&env));
        check(api->CreateSessionOptions(&options));
        check(api->SetIntraOpNumThreads(options,1));
        check(api->SetInterOpNumThreads(options,1));
        check(api->AddSessionConfigEntry(options,"session.intra_op.allow_spinning","0"));
        check(api->AddSessionConfigEntry(options,"session.inter_op.allow_spinning","0"));
        check(api->CreateSession(env,enroll_path,options,&enroll));
        check(api->CreateSession(env,chunk_path,options,&chunk));
        check(api->CreateCpuMemoryInfo(OrtArenaAllocator,OrtMemTypeDefault,&memory));
        int64_t shape[] = {1,48000};
        OrtValue *input = nullptr;
        check(api->CreateTensorWithDataAsOrtValue(memory,const_cast<float *>(reference),reference_count*sizeof(float),shape,2,ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT,&input));
        const char *inputs[] = {"enroll"}, *outputs[] = {"enroll_n","k","v"};
        const OrtValue *enroll_input[] = {input};
        OrtStatus *run_status = api->Run(enroll,nullptr,inputs,enroll_input,1,outputs,3,voice.data());
        api->ReleaseValue(input);
        check(run_status);
        OrtTypeInfo *type = nullptr;
        const OrtTensorTypeAndShapeInfo *tensor = nullptr;
        check(api->SessionGetInputTypeInfo(chunk,4,&type));
        try {
            check(api->CastTypeInfoToTensorInfo(type,&tensor));
            size_t n = 0;
            check(api->GetDimensionsCount(tensor,&n));
            if (n != 3) throw std::runtime_error("unexpected V2 recurrent state dimensions");
            std::vector<int64_t> dims(n);
            check(api->GetDimensions(tensor,dims.data(),n));
            size_t elements = 1;
            for (int64_t d : dims) {
                if (d <= 0 || d > 100000 || elements > 100000000 / static_cast<size_t>(d))
                    throw std::runtime_error("invalid V2 recurrent state dimensions");
                elements *= static_cast<size_t>(d);
            }
            state.assign(elements,0.0f);
            check(api->CreateTensorWithDataAsOrtValue(memory,state.data(),state.size()*sizeof(float),dims.data(),dims.size(),ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT,&h));
            check(api->CreateTensorWithDataAsOrtValue(memory,state.data(),state.size()*sizeof(float),dims.data(),dims.size(),ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT,&c));
        } catch (...) { api->ReleaseTypeInfo(type); throw; }
        api->ReleaseTypeInfo(type);
        ola.assign(512,0.0f); norm.assign(512,0.0f);
        constexpr double pi=3.14159265358979323846;
        for (int i=0;i<512;i++) { float w=static_cast<float>(0.5-0.5*std::cos(2*pi*i/512)); window[i]=w*w; }
    }
    std::vector<float> push(const float *samples, size_t count) {
        if (!samples || count==0 || count>160000) throw std::runtime_error("invalid input length");
        for (size_t i=0;i<count;i++) if (!std::isfinite(samples[i])) throw std::runtime_error("non-finite input");
        if (!started) {
            head.insert(head.end(),samples,samples+count);
            if (head.size()<=256) return {};
            for (int i=256;i>=1;i--) buf.push_back(head[static_cast<size_t>(i)]);
            buf.insert(buf.end(),head.begin(),head.end());
            head.clear(); started=true; drop=256;
        } else buf.insert(buf.end(),samples,samples+count);
        std::vector<float> result;
        while (buf.size()>=512) {
            const int frames=std::min<int>(100,1+static_cast<int>((buf.size()-512)/128));
            const int n=(frames-1)*128+512;
            int64_t shape[]={1,n};
            OrtValue *wav=nullptr;
            check(api->CreateTensorWithDataAsOrtValue(memory,buf.data(),static_cast<size_t>(n)*sizeof(float),shape,2,ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT,&wav));
            const char *names[]={"wav","enroll_n","k","v","h","c"};
            const char *outs[]={"frames","h_out","c_out"};
            const OrtValue *ins[]={wav,voice[0],voice[1],voice[2],h,c};
            OrtValue *vals[3]{};
            OrtStatus *run_status=api->Run(chunk,nullptr,names,ins,6,outs,3,vals);
            api->ReleaseValue(wav);
            if (run_status) { for (auto *v:vals) if (v) api->ReleaseValue(v); check(run_status); }
            if (!vals[0] || !vals[1] || !vals[2]) { for (auto *v:vals) if(v) api->ReleaseValue(v); throw std::runtime_error("model returned incomplete V2 state"); }
            OrtTensorTypeAndShapeInfo *info = nullptr;
            try {
                check(api->GetTensorTypeAndShape(vals[0],&info));
                size_t elements=0; check(api->GetTensorShapeElementCount(info,&elements));
                if (elements != static_cast<size_t>(frames)*512) throw std::runtime_error("invalid V2 output shape");
                ONNXTensorElementDataType element_type;
                check(api->GetTensorElementType(info,&element_type));
                if (element_type != ONNX_TENSOR_ELEMENT_DATA_TYPE_FLOAT) throw std::runtime_error("invalid V2 output type");
                float *frame_data=nullptr; check(api->GetTensorMutableData(vals[0],reinterpret_cast<void **>(&frame_data)));
                const int done=frames*128;
                ola.resize(512+done,0.0f); norm.resize(512+done,0.0f);
                for (int j=0;j<frames;j++) for (int i=0;i<512;i++) {
                    float value=frame_data[j*512+i];
                    if (!std::isfinite(value)) throw std::runtime_error("non-finite model output");
                    ola[j*128+i]+=value; norm[j*128+i]+=window[i];
                }
                const int cut=std::min(drop,done); drop-=cut;
                for(int i=cut;i<done;i++) result.push_back(-0.5f*ola[i]/(norm[i]>1e-11f?norm[i]:1.0f));
                ola.erase(ola.begin(),ola.begin()+done); norm.erase(norm.begin(),norm.begin()+done); buf.erase(buf.begin(),buf.begin()+done);
            } catch (...) { if(info) api->ReleaseTensorTypeAndShapeInfo(info); for (auto *v:vals) api->ReleaseValue(v); throw; }
            api->ReleaseTensorTypeAndShapeInfo(info);
            api->ReleaseValue(vals[0]); api->ReleaseValue(h); api->ReleaseValue(c);
            h=vals[1]; c=vals[2];
        }
        return result;
    }
};
void error_message(char *out, size_t capacity, const char *message) {
    if (!out || capacity==0) return;
    const size_t n=std::min(capacity-1,std::strlen(message));
    std::memcpy(out,message,n); out[n]='\0';
}
}

extern "C" {
__declspec(dllexport) void *vox_v2_create(const wchar_t *runtime, const wchar_t *enroll, const wchar_t *chunk,
                                          const float *reference, size_t reference_count, char *error, size_t error_size) {
    try { auto stream=std::make_unique<Stream>(); stream->init(runtime,enroll,chunk,reference,reference_count); return stream.release(); }
    catch (const std::exception &e) { error_message(error,error_size,e.what()); return nullptr; }
    catch (...) { error_message(error,error_size,"unknown V2 initialization error"); return nullptr; }
}
__declspec(dllexport) int vox_v2_push(void *handle, const float *samples, size_t count, float *output, size_t capacity,
                                     size_t *written, char *error, size_t error_size) {
    try {
        if (!handle || !output || !written) throw std::runtime_error("invalid V2 stream argument");
        *written=0;
        auto result=static_cast<Stream *>(handle)->push(samples,count);
        if (result.size()>capacity) throw std::runtime_error("V2 output buffer is too small; discard stream");
        std::copy(result.begin(),result.end(),output); *written=result.size(); return 0;
    } catch (const std::exception &e) { error_message(error,error_size,e.what()); return -1; }
    catch (...) { error_message(error,error_size,"unknown V2 processing error"); return -1; }
}
__declspec(dllexport) void vox_v2_destroy(void *handle) { delete static_cast<Stream *>(handle); }
}
