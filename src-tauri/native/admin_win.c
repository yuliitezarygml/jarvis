#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <shellapi.h>
#include <shlobj.h>
#include <sddl.h>
#include <wintrust.h>
#include <softpub.h>
#include <strsafe.h>

static TOKEN_USER *user_of(HANDLE process) {
    HANDLE token = NULL;
    DWORD size = 0;
    TOKEN_USER *user = NULL;
    if (!OpenProcessToken(process, TOKEN_QUERY, &token)) return NULL;
    GetTokenInformation(token, TokenUser, NULL, 0, &size);
    if (size && (user = LocalAlloc(LPTR,size)) != NULL &&
        !GetTokenInformation(token,TokenUser,user,size,&size)) {
        LocalFree(user);
        user = NULL;
    }
    CloseHandle(token);
    return user;
}
int jv_elevated(void) {
    TOKEN_ELEVATION elevation;
    HANDLE token;
    DWORD size;
    if (!OpenProcessToken(GetCurrentProcess(),TOKEN_QUERY,&token)) return -1;
    BOOL ok = GetTokenInformation(token,TokenElevation,&elevation,sizeof(elevation),&size);
    CloseHandle(token);
    return ok ? (elevation.TokenIsElevated != 0) : -1;
}
void *jv_pipe_descriptor(void) {
    TOKEN_USER *user = user_of(GetCurrentProcess());
    LPWSTR sid = NULL;
    WCHAR dacl[256];
    PSECURITY_DESCRIPTOR descriptor = NULL;
    if (user && ConvertSidToStringSidW(user->User.Sid,&sid) &&
        SUCCEEDED(StringCchPrintfW(dacl,256,L"D:P(A;;GA;;;%s)",sid)))
        ConvertStringSecurityDescriptorToSecurityDescriptorW(dacl,SDDL_REVISION_1,&descriptor,NULL);
    LocalFree(sid); LocalFree(user);
    return descriptor;
}
void jv_free_descriptor(void *descriptor) { LocalFree(descriptor); }

static BOOL trusted_image(const WCHAR *path) {
    WINTRUST_FILE_INFO fi = {0};
    WINTRUST_DATA wd = {0};
    GUID action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    fi.cbStruct = sizeof(fi); fi.pcwszFilePath = path;
    wd.cbStruct = sizeof(wd); wd.dwUIChoice = WTD_UI_NONE;
    wd.fdwRevocationChecks = WTD_REVOKE_WHOLECHAIN;
    wd.dwUnionChoice = WTD_CHOICE_FILE; wd.pFile = &fi;
    wd.dwStateAction = WTD_STATEACTION_VERIFY;
    wd.dwProvFlags = WTD_REVOCATION_CHECK_CHAIN_EXCLUDE_ROOT | WTD_CACHE_ONLY_URL_RETRIEVAL;
    LONG status = WinVerifyTrust(INVALID_HANDLE_VALUE,&action,&wd);
    wd.dwStateAction = WTD_STATEACTION_CLOSE;
    WinVerifyTrust(INVALID_HANDLE_VALUE,&action,&wd);
    return status == ERROR_SUCCESS;
}
static BOOL own_path(WCHAR *path,DWORD capacity) {
    DWORD n = GetModuleFileNameW(NULL,path,capacity);
    return n && n < capacity;
}
int jv_safe_install(void) {
    WCHAR path[32768], program_files[MAX_PATH];
    if (!own_path(path,32768) || FAILED(SHGetFolderPathW(NULL,CSIDL_PROGRAM_FILES,NULL,SHGFP_TYPE_CURRENT,program_files))) return 0;
    size_t n = wcslen(program_files);
    return !_wcsnicmp(path,program_files,n) && path[n]==L'\\' && trusted_image(path);
}
int jv_verify_peer(HANDLE pipe,DWORD expected,int peer_is_client) {
    ULONG actual = 0;
    if (!(peer_is_client ? GetNamedPipeClientProcessId(pipe,&actual) : GetNamedPipeServerProcessId(pipe,&actual)) || actual != expected) return 0;
    HANDLE process = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,FALSE,actual);
    if (!process) return 0;
    WCHAR image[32768], own[32768];
    DWORD n = 32768;
    TOKEN_USER *me = user_of(GetCurrentProcess()), *peer = user_of(process);
    BOOL ok = me && peer && EqualSid(me->User.Sid,peer->User.Sid) && own_path(own,32768) &&
        QueryFullProcessImageNameW(process,0,image,&n) && !_wcsicmp(image,own) && trusted_image(image);
    LocalFree(me); LocalFree(peer); CloseHandle(process);
    return ok;
}
HANDLE jv_runas(const WCHAR *args) {
    WCHAR path[32768];
    if (!own_path(path,32768) || !jv_safe_install()) return NULL;
    SHELLEXECUTEINFOW info = {0};
    info.cbSize=sizeof(info);
    info.fMask=SEE_MASK_NOCLOSEPROCESS | SEE_MASK_NOASYNC;
    info.lpVerb=L"runas"; info.lpFile=path; info.lpParameters=args; info.nShow=SW_HIDE;
    return ShellExecuteExW(&info) ? info.hProcess : NULL;
}
int jv_security_log_count(DWORD *count) {
    HANDLE log = OpenEventLogW(NULL,L"Security");
    if (!log) return 0;
    BOOL ok = GetNumberOfEventLogRecords(log,count);
    CloseEventLog(log);
    return ok;
}
