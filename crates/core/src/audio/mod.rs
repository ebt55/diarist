//! Audio capture and pipeline (Phases 1 and 3).
//!
//! File mode: decode any container (`symphonia`) to 16 kHz mono PCM16 WAV.
//! Live mode: WASAPI mic and render-endpoint loopback capture into lock-free
//! rings, QPC timeline alignment, zero-fill of gaps and SILENT packets,
//! 48 to 16 kHz resampling (`rubato`), per-channel WAV writers, and 100 ms
//! frames handed to the engine. Real-time callbacks only copy; all work happens
//! on a worker thread.
