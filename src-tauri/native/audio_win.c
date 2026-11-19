#define WIN32_LEAN_AND_MEAN
#include <windows.h>
#include <mmsystem.h>
#include <stdint.h>
#include <string.h>

// Callers poll at the existing Rust capture/playback cadence. WinMM owns each
// queued header until WHDR_DONE or reset. Storage stays at a stable address.
enum { INPUT_BLOCKS = 32, FRAME = 256, OUTPUT_BLOCKS = 5, OUTPUT_FRAME = 1920 };
static SRWLOCK gate = SRWLOCK_INIT;
static HWAVEIN mic;
static HWAVEOUT speaker;
static WAVEHDR input[INPUT_BLOCKS], output[OUTPUT_BLOCKS];
static int16_t samples[INPUT_BLOCKS][FRAME], pcm[OUTPUT_BLOCKS][OUTPUT_FRAME];
static unsigned next_input, consumed;

static WAVEFORMATEX format(DWORD rate) {
    WAVEFORMATEX f = {WAVE_FORMAT_PCM, 1, rate, rate * 2, 2, 16, 0};
    return f;
}
static void close_mic(void) {
    if (!mic) return;
    waveInReset(mic);
    for (unsigned i = 0; i < INPUT_BLOCKS; i++)
        if (input[i].dwFlags & WHDR_PREPARED) waveInUnprepareHeader(mic, &input[i], sizeof(WAVEHDR));
    waveInClose(mic);
    mic = NULL;
}
int jv_mic_start(int continuous) {
    (void)continuous;
    AcquireSRWLockExclusive(&gate);
    close_mic();
    WAVEFORMATEX f = format(16000);
    MMRESULT result = waveInOpen(&mic, WAVE_MAPPER, &f, 0, 0, CALLBACK_NULL);
    ZeroMemory(input, sizeof(input));
    next_input = consumed = 0;
    for (unsigned i = 0; result == MMSYSERR_NOERROR && i < INPUT_BLOCKS; i++) {
        input[i].lpData = (LPSTR)samples[i];
        input[i].dwBufferLength = sizeof(samples[i]);
        result = waveInPrepareHeader(mic, &input[i], sizeof(WAVEHDR));
        if (!result) result = waveInAddBuffer(mic, &input[i], sizeof(WAVEHDR));
    }
    if (!result) result = waveInStart(mic);
    if (result) close_mic();
    ReleaseSRWLockExclusive(&gate);
    return result == MMSYSERR_NOERROR ? 1 : (result == MMSYSERR_NOTENABLED ? -1 : 0);
}
int jv_mic_status(void) {
    // Availability without opening a microphone or inferring privacy consent.
    return waveInGetNumDevs() ? 0 : -1;
}
void jv_mic_end(void) {
    AcquireSRWLockExclusive(&gate);
    close_mic();
    ReleaseSRWLockExclusive(&gate);
}
int jv_read(float *out, int cap) {
    int count = 0;
    AcquireSRWLockExclusive(&gate);
    while (mic && count < cap && (input[next_input].dwFlags & WHDR_DONE)) {
        WAVEHDR *h = &input[next_input];
        while (count < cap && consumed < h->dwBytesRecorded / 2)
            out[count++] = samples[next_input][consumed++] / 32768.0f;
        if (consumed < h->dwBytesRecorded / 2) break;
        if (waveInAddBuffer(mic, h, sizeof(*h))) { close_mic(); break; }
        next_input = (next_input + 1) % INPUT_BLOCKS;
        consumed = 0;
    }
    ReleaseSRWLockExclusive(&gate);
    return count;
}
int jv_play(const int16_t *data, int n) {
    if (n <= 0 || n > OUTPUT_FRAME) return -1;
    int accepted = 0;
    AcquireSRWLockExclusive(&gate);
    if (!speaker) {
        WAVEFORMATEX f = format(24000);
        if (waveOutOpen(&speaker, WAVE_MAPPER, &f, 0, 0, CALLBACK_NULL)) accepted = -1;
    }
    for (unsigned i = 0; speaker && i < OUTPUT_BLOCKS; i++) {
        WAVEHDR *h = &output[i];
        if (h->dwFlags & WHDR_INQUEUE) continue;
        if ((h->dwFlags & WHDR_PREPARED) && waveOutUnprepareHeader(speaker, h, sizeof(*h))) continue;
        ZeroMemory(h, sizeof(*h));
        memcpy(pcm[i], data, n * sizeof(*data));
        h->lpData = (LPSTR)pcm[i];
        h->dwBufferLength = n * sizeof(*data);
        accepted = (!waveOutPrepareHeader(speaker,h,sizeof(*h)) && !waveOutWrite(speaker,h,sizeof(*h))) ? 1 : -1;
        break;
    }
    ReleaseSRWLockExclusive(&gate);
    return accepted;
}
int jv_pending(void) {
    int count = 0;
    AcquireSRWLockShared(&gate);
    for (unsigned i = 0; i < OUTPUT_BLOCKS; i++) count += !!(output[i].dwFlags & WHDR_INQUEUE);
    ReleaseSRWLockShared(&gate);
    return count;
}
void jv_stop(void) {
    AcquireSRWLockExclusive(&gate);
    if (speaker) {
        waveOutReset(speaker);
        for (unsigned i = 0; i < OUTPUT_BLOCKS; i++)
            if (output[i].dwFlags & WHDR_PREPARED) waveOutUnprepareHeader(speaker,&output[i],sizeof(WAVEHDR));
        waveOutClose(speaker);
        speaker = NULL;
        ZeroMemory(output,sizeof(output));
    }
    ReleaseSRWLockExclusive(&gate);
}
void jv_gpu(char *out, int n) {
    DISPLAY_DEVICEW d = {0};
    d.cb = sizeof(d);
    if (!EnumDisplayDevicesW(NULL,0,&d,0) ||
        !WideCharToMultiByte(CP_UTF8,0,d.DeviceString,-1,out,n,NULL,NULL)) out[0] = 0;
}
