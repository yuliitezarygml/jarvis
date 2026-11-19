#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <mmdeviceapi.h>
#include <endpointvolume.h>
#include <wrl/client.h>
#include <shobjidl.h>
#include <shellapi.h>
#include <cmath>

// Use the default multimedia rendering endpoint. A negative argument means read.
extern "C" int jv_endpoint_volume(int requested) {
    using Microsoft::WRL::ComPtr;
    HRESULT init = CoInitializeEx(nullptr, COINIT_MULTITHREADED);
    if (FAILED(init) && init != RPC_E_CHANGED_MODE) return -1;
    int result = -1;
    {
        ComPtr<IMMDeviceEnumerator> devices;
        ComPtr<IMMDevice> device;
        ComPtr<IAudioEndpointVolume> volume;
        if (SUCCEEDED(CoCreateInstance(__uuidof(MMDeviceEnumerator),nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&devices))) &&
            SUCCEEDED(devices->GetDefaultAudioEndpoint(eRender,eMultimedia,&device)) &&
            SUCCEEDED(device->Activate(__uuidof(IAudioEndpointVolume),CLSCTX_INPROC_SERVER,nullptr,
                                       reinterpret_cast<void**>(volume.GetAddressOf())))) {
            float level = 0;
            if ((requested < 0 || SUCCEEDED(volume->SetMasterVolumeLevelScalar(requested/100.0f,nullptr))) &&
                SUCCEEDED(volume->GetMasterVolumeLevelScalar(&level)))
                result = static_cast<int>(std::lround(level*100));
        }
    }
    if (SUCCEEDED(init)) CoUninitialize();
    return result;
}

static BOOL CALLBACK request_close(HWND window, LPARAM pid) {
    DWORD owner = 0;
    GetWindowThreadProcessId(window,&owner);
    if (owner == static_cast<DWORD>(pid) && IsWindowVisible(window))
        PostMessageW(window,WM_CLOSE,0,0);
    return TRUE;
}
extern "C" void jv_close_app(DWORD pid) {
    EnumWindows(request_close,static_cast<LPARAM>(pid));
}
extern "C" long jv_recycle(const wchar_t *path) {
    using Microsoft::WRL::ComPtr;
    HRESULT init=CoInitializeEx(nullptr,COINIT_APARTMENTTHREADED);
    if (FAILED(init)) return init;
    HRESULT hr;
    {
        ComPtr<IFileOperation> operation;
        ComPtr<IShellItem> item;
        hr=CoCreateInstance(CLSID_FileOperation,nullptr,CLSCTX_INPROC_SERVER,IID_PPV_ARGS(&operation));
        if (SUCCEEDED(hr)) hr=operation->SetOperationFlags(FOFX_RECYCLEONDELETE|FOFX_EARLYFAILURE|FOF_SILENT|FOF_NOCONFIRMATION|FOF_NOERRORUI);
        if (SUCCEEDED(hr)) hr=SHCreateItemFromParsingName(path,nullptr,IID_PPV_ARGS(&item));
        if (SUCCEEDED(hr)) hr=operation->DeleteItem(item.Get(),nullptr);
        if (SUCCEEDED(hr)) hr=operation->PerformOperations();
        BOOL aborted=FALSE;
        if (SUCCEEDED(hr)) hr=operation->GetAnyOperationsAborted(&aborted);
        if (aborted) hr=HRESULT_FROM_WIN32(ERROR_CANCELLED);
    }
    CoUninitialize();
    return hr;
}
