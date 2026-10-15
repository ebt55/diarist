//! Windows Media Foundation decode: `IMFSourceReader` to interleaved float at
//! the source rate, then [`Pcm16kWriter`] for the downmix and resampling.

use std::path::Path;

use windows::core::HSTRING;
use windows::Win32::Media::MediaFoundation::{
    IMFMediaType, IMFSourceReader, MFAudioFormat_Float, MFCreateMediaType,
    MFCreateSourceReaderFromURL, MFMediaType_Audio, MFShutdown, MFStartup, MFSTARTUP_NOSOCKET,
    MF_MT_AUDIO_NUM_CHANNELS, MF_MT_AUDIO_SAMPLES_PER_SECOND, MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE,
    MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READER_ALL_STREAMS,
    MF_SOURCE_READER_FIRST_AUDIO_STREAM, MF_VERSION,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};

use super::decode::Pcm16kWriter;
use super::DecodedInfo;
use crate::error::{CoreError, Result};

fn mf_err(what: &str, e: windows::core::Error) -> CoreError {
    CoreError::Decode(format!("media foundation {what}: {e}"))
}

/// Balances `CoInitializeEx`/`MFStartup` on every exit path.
struct MfSession {
    com: bool,
}

impl MfSession {
    fn start() -> Result<Self> {
        // S_FALSE (already initialised) and RPC_E_CHANGED_MODE (another
        // apartment) both leave COM usable for MF on this thread.
        let hr = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };
        let com = hr.is_ok();
        unsafe { MFStartup(MF_VERSION, MFSTARTUP_NOSOCKET) }.map_err(|e| {
            if com {
                unsafe { CoUninitialize() };
            }
            mf_err("startup", e)
        })?;
        Ok(Self { com })
    }
}

impl Drop for MfSession {
    fn drop(&mut self) {
        unsafe {
            let _ = MFShutdown();
            if self.com {
                CoUninitialize();
            }
        }
    }
}

pub(super) fn decode(input: &Path, output: &Path) -> Result<DecodedInfo> {
    let _session = MfSession::start()?;
    let first_audio = MF_SOURCE_READER_FIRST_AUDIO_STREAM.0 as u32;
    let path = std::path::absolute(input)?;
    let reader: IMFSourceReader =
        unsafe { MFCreateSourceReaderFromURL(&HSTRING::from(path.as_os_str()), None) }
            .map_err(|e| mf_err("open", e))?;
    unsafe {
        reader
            .SetStreamSelection(MF_SOURCE_READER_ALL_STREAMS.0 as u32, false)
            .map_err(|e| mf_err("select streams", e))?;
        reader
            .SetStreamSelection(first_audio, true)
            .map_err(|e| mf_err("select audio", e))?;
        let wanted: IMFMediaType = MFCreateMediaType().map_err(|e| mf_err("media type", e))?;
        wanted
            .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Audio)
            .map_err(|e| mf_err("media type", e))?;
        wanted
            .SetGUID(&MF_MT_SUBTYPE, &MFAudioFormat_Float)
            .map_err(|e| mf_err("media type", e))?;
        reader
            .SetCurrentMediaType(first_audio, None, &wanted)
            .map_err(|e| mf_err("set float output", e))?;
    }
    let current = unsafe { reader.GetCurrentMediaType(first_audio) }
        .map_err(|e| mf_err("current type", e))?;
    let rate = unsafe { current.GetUINT32(&MF_MT_AUDIO_SAMPLES_PER_SECOND) }
        .map_err(|e| mf_err("sample rate", e))?;
    let channels = unsafe { current.GetUINT32(&MF_MT_AUDIO_NUM_CHANNELS) }
        .map_err(|e| mf_err("channels", e))?;
    let mut writer = Pcm16kWriter::create(output, rate, channels as usize)?;
    let mut floats: Vec<f32> = Vec::new();
    loop {
        let mut flags = 0u32;
        let mut sample = None;
        unsafe {
            reader.ReadSample(
                first_audio,
                0,
                None,
                Some(&mut flags),
                None,
                Some(&mut sample),
            )
        }
        .map_err(|e| mf_err("read", e))?;
        if let Some(sample) = sample {
            let buffer =
                unsafe { sample.ConvertToContiguousBuffer() }.map_err(|e| mf_err("buffer", e))?;
            let mut ptr: *mut u8 = std::ptr::null_mut();
            let mut len = 0u32;
            unsafe { buffer.Lock(&mut ptr, None, Some(&mut len)) }
                .map_err(|e| mf_err("lock", e))?;
            let n = len as usize / std::mem::size_of::<f32>();
            floats.clear();
            if !ptr.is_null() && n > 0 {
                // SAFETY: the locked buffer holds `len` bytes of f32 samples; we
                // copy them out before unlocking (alignment is not guaranteed).
                let bytes = unsafe { std::slice::from_raw_parts(ptr, n * 4) };
                floats.extend(
                    bytes
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .map(|b| f32::from_le_bytes(*b)),
                );
            }
            unsafe { buffer.Unlock() }.map_err(|e| mf_err("unlock", e))?;
            writer.push_interleaved(&floats)?;
        }
        if flags & (MF_SOURCE_READERF_ENDOFSTREAM.0 as u32) != 0 {
            break;
        }
    }
    writer.finish(channels as u16)
}
