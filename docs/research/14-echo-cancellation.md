# Echo cancellation: measurements

Measurements behind `docs/plans/15-echo-without-headphones.md` section 3.3: can Windows' own acoustic echo cancellation clean Diarist's mic channel when the meeting plays on speakers, and what does it cost. No product code changed. The probe is a hidden developer subcommand, `diarist-cli aec-probe` (`crates/cli/src/aec_probe.rs`). Opus 5.5 implementation agent.

## 1. OS echo cancellation probe (20 Oct 2026)

**Summary** (the numbers are in 1.4 to 1.6):

- On this PC, `IAcousticEchoCancellationControl` is offered only to capture streams in the Speech family of categories: Speech, FarFieldSpeech, UniformSpeech and VoiceTyping. It is offered on all three capture endpoints, including the QuadCast and the VB-Audio virtual cable. It is not offered with no category (what the app does today), with Communications or with GameChat. On a Speech-family stream, the effect list reports AEC, NS and AGC, the reference endpoint is accepted, and `is_aec_supported` is true.
- **No echo was cancelled.** On a digital loop (the VB-Audio cable, with its own render side as the AEC reference), the processed mic is still the reference: normalised correlation 0.93, and 11.6 dB louder than unprocessed, with no convergence over 12 s. With the QuadCast and the speakers, the echo bursts in a processed take stand 14 to 15 dB above the gaps between them. An unprocessed take gives 13 dB, so there is no sign of cancellation.
- What the Speech-family processing did do:
  - **Gain:** +11.6 dB on the cable echo. In silent takes on the QuadCast, the level went from -42 dBFS unprocessed to -20 to -22 dBFS processed, with peaks up to -3.9 dBFS.
  - **Delay:** 110 to 130 ms more delay in the audio content. Packet timestamps, packet rate and the age of packets at read time do not show it.
- **Ducking:** No ducking was measured. The loopback level of a tone played by another process changed by 0.0 dB while capture streams in any category were open, on the default communications capture endpoint and on the QuadCast.
- **Changes on a running stream:**
  - Setting the AEC reference to another endpoint, and back, returns S_OK.
  - The AEC effect state is read-only (`AUDCLNT_E_EFFECT_STATE_READ_ONLY`).

### 1.1 Setup

| Item | Value |
|---|---|
| Machine | Windows 11 Pro 25H2, build 26200.9711; RTX 3080 desktop |
| Mic under test | Microphone (HyperX QuadCast), USB. Windows endpoint volume +4.8 dB (scalar 0.87), not muted in Windows. Its own hardware mute was on for the first runs (1.6.6) |
| Speakers | Realtek Digital Output (Realtek(R) Audio), the default console render endpoint, at -7.2 dB (scalar 0.62) when read during the later runs |
| Default endpoints at probe time | Capture, console role: QuadCast. Capture, **communications role: CABLE Output (2- VB-Audio Virtual Cable)**. Render, console role: Realtek Digital Output. Render, communications role: Speakers (HyperX QuadCast). The app's mic default is the communications capture endpoint, so `diarist-cli record` without `--mic` opens the VB-Audio cable on this PC |
| Other endpoints | Capture: Microphone (3- Razer Kiyo). Render: Speakers (2- VB-Audio Virtual Cable), which is the cable's render side; CABLE In 16 Ch (2- VB-Audio Virtual Cable) |
| Endpoint APOs | No APO is registered on the QuadCast endpoint: `MMDevices\Audio\Capture\{ad43fc28-…}\FxProperties` holds only an APO settings store (Default, User, Volatile) and no APO CLSID values. No Voice Clarity or APO device appears in PnP |
| Ducking setting | `HKCU\Software\Microsoft\Multimedia\Audio\UserDuckingPreference` is absent, which is the Sound settings default "reduce other sounds by 80%" (value meanings from secondary sources; unconfirmed) |
| Probe | `diarist-cli aec-probe` on wasapi 0.26.0 and windows 0.62.2, debug build |

### 1.2 What Microsoft documents (Part A)

Each answer gives what is documented first, then what this PC showed (sections 1.4 to 1.6), and marks the gaps. The quotes are short excerpts from the cited pages.

**Q1. What does `SetEchoCancellationRenderEndpoint` require of the capture stream? Which build introduced it?**

- **Documented:**
  - [`IAcousticEchoCancellationControl::SetEchoCancellationRenderEndpoint`](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iacousticechocancellationcontrol-setechocancellationrenderendpoint): minimum client **Windows Build 22621** (22H2). The interface is obtained with `IAudioClient::GetService` after `Initialize`. If `GetService` fails with `E_NOINTERFACE`, "the AEC effect on the endpoint (if supported), does not allow control over the loopback reference endpoint". The page also says "an endpoint may support AEC, but may not support control of loopback reference endpoint". It states no stream-category requirement.
  - [`IAudioEffectsManager::SetAudioEffectState`](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioeffectsmanager-setaudioeffectstate): the effects manager needs Build 22000.
  - [Audio signal processing modes](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/audio-signal-processing-modes): an app's category is mapped to a driver-defined mode, which apps cannot see or change. If the driver lacks a mode, "Windows will use the next best matching mode". The Speech mode is "Human voice capture" and the Communications mode is VOIP render and capture. `SetClientProperties` must come after activation and before `Initialize` (code comment on the same page).
  - [Windows 11 APIs for APOs](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/windows-11-apis-for-audio-processing-objects): an AEC APO "may only be implemented by mode effects (MFX) on capture endpoints", so AEC belongs to whichever mode the APO is registered for.
  - [`AUDIO_STREAM_CATEGORY`](https://learn.microsoft.com/en-us/windows/win32/api/audiosessiontypes/ne-audiosessiontypes-audio_stream_category): the valid capture categories are Communications, Speech and Other.
  - The official [AcousticEchoCancellation sample](https://learn.microsoft.com/en-us/samples/microsoft/windows-classic-samples/acousticechocancellation/) is described as giving "applications using communications streams" control of the reference. By default the system uses the default render device, and the sample needs build 22540 or later. The wasapi crate's `aec` example sets `StreamCategory::Communications`.
- **On this PC:**
  - The control and the AEC effect exist only for Speech, FarFieldSpeech, UniformSpeech and VoiceTyping. With Communications, `GetService` returns `E_NOINTERFACE` and the stream lists no effects at all (1.4).
  - `SetClientProperties` with GameChat, FarFieldSpeech, UniformSpeech and VoiceTyping was accepted on capture streams, and `Initialize` succeeded, even though the enum page lists only three valid capture categories.
- **Unconfirmed:**
  - Which component supplies the Speech-family AEC/NS/AGC here.
  - Whether `SetClientProperties` after `Initialize` fails; the probe always called it before.

**Q2. Does a Communications capture stream trigger Windows' default ducking?**

- **Documented:**
  - [Using a communication device](https://learn.microsoft.com/en-us/windows/win32/coreaudio/using-the-communication-device): "The operating system considers the stream opened on a communication device to be a communication stream". Also: "The audio system generates ducking events when a communication stream is opened or closed for rendering or capturing streams". So the documented trigger is the device role (eCommunications), and capture streams are included.
  - [Default ducking experience](https://learn.microsoft.com/en-us/windows/win32/coreaudio/stream-attenuation): the default attenuation is 80%. Volume returns "When all of the communication streams are closed".
  - [`AUDIO_STREAM_CATEGORY`](https://learn.microsoft.com/en-us/windows/win32/api/audiosessiontypes/ne-audiosessiontypes-audio_stream_category): GameChat is "Similar to AudioCategory_Communications except that AudioCategory_GameChat will not attenuate other streams". It is listed for render only.
  - The signal processing modes page says "tagging a stream as Communications will let Windows know to pause background music".
  - These pages do not separate render and capture any further.
- **On this PC (1.6.5):**
  - While capture streams were open with no category, Communications, Speech or GameChat, the endpoint loopback level of a tone played by another process did not change (0.0 dB).
  - This held for capture on the default communications capture endpoint (CABLE Output) and on the QuadCast, which is not the default communications endpoint.
  - The app today already opens the default communications capture endpoint with no category. That case also showed 0.0 dB.
- **Unconfirmed:**
  - Why this PC shows no ducking when the role-based description predicts it for capture.
  - That ducking would show in the endpoint loopback. The probe assumes ducking is applied as session attenuation before the loopback tap. The Volume Mixer was not observed.
  - Render streams in the Communications category were not tested.

**Q3. Can the reference endpoint change while the stream runs, and can AEC be turned off on a running client?**

- **Documented:**
  - Passing NULL makes "Windows using its own algorithm to pick the loopback reference device". An invalid id returns `E_INVALIDARG` ([`SetEchoCancellationRenderEndpoint`](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iacousticechocancellationcontrol-setechocancellationrenderendpoint)). The page says nothing about calling it on a running stream.
  - The APO page describes the engine swapping the reference input on a render device change (RemoveAuxiliaryInput, then AddAuxiliaryInput). It also says "When there is no audio being rendered out the render endpoint, loopback data will not be available at the auxiliary input" ([Windows 11 APIs for APOs](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/windows-11-apis-for-audio-processing-objects)).
  - [`SetAudioEffectState`](https://learn.microsoft.com/en-us/windows/win32/api/audioclient/nf-audioclient-iaudioeffectsmanager-setaudioeffectstate) returns `AUDCLNT_E_EFFECT_STATE_READ_ONLY` when "The specified effect has a state that is read-only". An APO must implement `IAudioSystemEffects3` for its effects to be switchable (signal processing modes page).
- **On this PC (1.6.3):**
  - On a running Speech stream, setting the reference to another render endpoint and back to the original returned S_OK both times. This was done in three takes.
  - Whether the change took effect could not be observed, because no cancellation was observed at all.
  - `SetAudioEffectState(AEC, OFF)` returned `0x88890042` (`AUDCLNT_E_EFFECT_STATE_READ_ONLY`). The AEC, NS and AGC effects all report `canSetState` false on every endpoint.
  - NULL was not exercised.

**Q4. Is a Microsoft APO available on any capture endpoint? What constraints apply?**

- **Documented:**
  - Voice Clarity ([Windows Insider build 26040 announcement](https://blogs.windows.com/windows-insider/2024/01/26/announcing-windows-11-insider-preview-build-26040-canary-channel/)) "cancels echo, suppresses background noise, and reduces reverberation in real-time". It applies to apps that use Communications Signal Processing Mode "when the OEM device does not offer Communications Mode processing", on x64 and Arm64 without an NPU. It is listed among the [24H2 features](https://blogs.windows.com/windows-insider/2024/05/22/releasing-windows-11-version-24h2-to-the-release-preview-channel/) ("voice clarity and more"). Secondary press covers its expansion beyond Surface, for example [Windows Central](https://www.windowscentral.com/software-apps/windows-11/all-windows-11-pcs-will-soon-support-this-ai-feature-once-exclusive-to-surface-no-npu-required).
  - The [USB Audio 2.0 driver](https://learn.microsoft.com/en-us/windows-hardware/drivers/audio/usb-2-0-audio-drivers) page mentions no APO, and says "There isn't any specific partner customization" for the in-box driver. Whether the QuadCast runs on usbaudio2.sys or on the USB Audio 1.0 driver was not checked.
  - The APO page says the engine resamples the reference to the AEC APO's preferred format. The reference is a pre-volume loopback by default; post-volume loopback is optional and not available on every endpoint.
  - No Learn page gives a latency figure.
- **On this PC (1.4):**
  - Communications-category streams list no effects on any endpoint, so no Communications-mode AEC (Voice Clarity or other) is in effect here.
  - The Speech family lists AEC, NS and AGC on the QuadCast, the Razer Kiyo and the VB-Audio virtual cable. None of them has an APO registered in its endpoint properties, so the processing comes from Windows; which component is unconfirmed.
  - It applies to the app's own format: 16 kHz mono PCM16 with AUTOCONVERTPCM, from a 48 kHz 2-channel float mix format.
  - The mic does not have to be the default communications endpoint: the QuadCast and the Kiyo report the same as the default one.
  - The measured cost is 110 to 130 ms of added content delay (1.6.4).

**Q5. How does it behave under double talk?**

- **Documented:** nothing found from Microsoft for the OEM AEC, Voice Clarity or the Speech-mode effect (unconfirmed).
- **On this PC (1.6.2, digital loop):** with the Speech-family AEC on, the far end passed at +9 to +10.6 dB, which is the AGC gain, in every segment. The near end passed during double talk at +10.7 dB, against +6.5 dB alone. So no near-end attenuation was seen, but no far-end cancellation happened either. This says nothing about how a canceller that works treats double talk.

### 1.3 Probe method (Part B)

`diarist-cli aec-probe --out <folder> [--seconds 12] [--mic <id or name>] [--render <id or name>] [--category communications|speech|other|gamechat] [--wav <16 kHz 16-bit WAV>] [--ducking] [--double-talk]`

The probe is hidden from `--help` and lives only in the CLI. It uses diarist-core's public `list_devices`, `play_wav` and `qpc_now_100ns`, plus its own WASAPI clients. It does not touch `audio/capture`.

- **a. Support table.**
  - For every capture endpoint and seven categories, a client is opened the way the app opens its mic: shared mode, polling, 200 ms buffer, 16 kHz mono PCM16, AUTOCONVERTPCM and SRC_DEFAULT_QUALITY.
  - The seven categories are none (the app), Communications, Speech, GameChat, FarFieldSpeech, UniformSpeech and VoiceTyping.
  - `SetClientProperties` comes before `Initialize`. After `Initialize`, the probe records `is_aec_supported`, `GetService(IAcousticEchoCancellationControl)` and `IAudioEffectsManager::GetAudioEffects`.
- **b. Reference signal.** 12 s at 16 kHz of band-limited noise bursts, 300 ms on and 200 ms off. The centre frequencies cycle through 500, 1,400, 800, 2,400, 650, 1,800, 1,000 and 3,000 Hz (two cascaded RBJ band-passes, Q 1.4), with 15 ms ramps, peak -12 dBFS and RMS -28.3 dBFS. It is written to `ref.wav`.
- **c. Run 1, AEC off.** The mic is opened as the app does (no category) and recorded for 1 s. Then `play_wav` plays `ref.wav` on the render endpoint, then 1 s more. Saved as `mic-aec-off.wav`.
- **d. Run 2, AEC on.** Same, with `--category` set before `Initialize` and the render endpoint set as the AEC reference after it (when the control exists). Saved as `mic-aec-on.wav`.
- **e. Run 3.** 5 s with the same AEC-on client and nothing playing. Saved as `mic-aec-on-silence.wav`.
- **Run 4, run-time control.** Run only when run 2 had the control. At 1/4 of the playback, the reference is switched to another render endpoint; at 1/2 it is set back; at 3/4, `SetAudioEffectState(AEC, OFF)` is called. Saved as `mic-aec-controls.wav`.
- **`--double-talk`.** The mic is `CABLE Output` and `--render` is the cable's render side. The default output plays the far end (the reference). The cable carries the same far end 60 ms later at -6 dB, plus a different speech-like near-end signal (bursts of 250 ms on and 250 ms off at other centre frequencies) from 4 to 8 s, and alone after the far end.
- **`--ducking`.** A second process (`diarist-cli play`) plays a 330 Hz tone at -24 dBFS on the default output. The probe loops that endpoint back and opens the mic for 3 s in each category (none, Communications, Speech, GameChat) with 2 s gaps.
- **f. Metrics.**
  - Timeline: mic samples are placed on QPC time from the first packet's timestamp. The playback start comes from the render device clock (`PlayReport`).
  - Mic RMS (dBFS, full-scale square wave = 0) during the playback span, shifted by the run's lag, and outside it (pre-roll and post-roll, the post-roll starting 300 ms after the span).
  - ERLE as asked: run 1 minus run 2 during playback.
  - Lag: FFT cross-correlation of the mic against the reference, -100 to +500 ms, normalised.
  - Burst contrast: mic power in the reference's bursts (edges trimmed) over mic power in the late part of the gaps (from 100 ms after a burst), at the lag in -20 to +400 ms that maximises it. Both sides come from the same take, so the result does not depend on any gain the processing applies.
  - Least-squares gain of the mic on the aligned reference, and the remainder after it.
  - Packet timing: first packet after `Start`, packets per second, frames per packet, the age of each packet at read time (QPC at read minus the packet timestamp), `GetStreamLatency`, device period, and flags.
  - Endpoint volume and mute (`IAudioEndpointVolume`).
- **g.** If the mic during playback in run 1 is within 3 dB of the mic outside it, the probe says the mic is probably muted (or the speakers are off), skips runs 2 to 4 and reports API support only.

The probe was extended between invocations: endpoint volume, per-run lag and burst contrast were added after the cable run. Each number below names the invocation it comes from (1.7). Two analyses were done offline from the saved WAVs with scratchpad scripts, and are marked "(offline)":
- The burst-envelope alignment.
- The double-talk table. In-run, the cable invocation aligned that table by the unprocessed lag; the probe now aligns by the processed take's own lag.

### 1.4 AEC support per endpoint and category

All six invocations gave the same table. Mix format: 48 kHz, 2 channels, 32-bit float on all three endpoints. "Control" is `GetService(IAcousticEchoCancellationControl)`.

| Capture endpoint | none (app) | Communications | Speech | GameChat | FarFieldSpeech | UniformSpeech | VoiceTyping |
|---|---|---|---|---|---|---|---|
| Microphone (HyperX QuadCast) [default] | no; control E_NOINTERFACE; effects none | no; E_NOINTERFACE; none | **yes**; control ok; AEC, NS, AGC on | no; E_NOINTERFACE; none | **yes**; ok; AEC, NS, AGC | **yes**; ok; AEC, NS, AGC | **yes**; ok; AEC, NS, AGC |
| Microphone (3- Razer Kiyo) | no; E_NOINTERFACE; none | no; E_NOINTERFACE; none | **yes**; ok; AEC, NS, AGC | no; E_NOINTERFACE; none | **yes** | **yes** | **yes** |
| CABLE Output (2- VB-Audio Virtual Cable) [default comms] | no; E_NOINTERFACE; none | no; E_NOINTERFACE; none | **yes**; ok; AEC, NS, AGC | no; E_NOINTERFACE; none | **yes** | **yes** | **yes** |

Every listed effect reports state on and `canSetState` false. SetClientProperties and Initialize succeeded in every cell.

### 1.5 Packet timing and latency

| Invocation, run | First packet after Start | Its timestamp after Start | Packets/s | Frames/packet | Age at read, median / p95 | GetStreamLatency |
|---|---|---|---|---|---|---|
| QuadCast, Speech (Q-S2), run 1 off | 31.2 ms | +12.0 ms | 99.7 | 160 | 22.4 / 27.2 ms | 0 |
| Q-S2, run 2 Speech | 30.7 ms | +11.9 ms | 99.8 | 160 | 22.6 / 27.3 ms | 0 |
| Q-S2, run 3 Speech, silence | 31.4 ms | +12.3 ms | 99.3 | 160 | 23.2 / 27.8 ms | 0 |
| QuadCast, Communications (Q-C2), run 2 | 31.4 ms | +12.9 ms | 99.7 | 160 | 22.2 / 27.1 ms | 0 |
| Cable, Speech (C-S), run 1 off | 20.5 ms | +0.4 ms | 99.8 | 160 | 18.8 / 29.6 ms | 0 |
| C-S, run 2 Speech | 20.3 ms | +0.4 ms | 99.9 | 160 | 17.2 / 29.3 ms | 0 |

Device period was 10.0 ms (default) / 3.0 ms (minimum) everywhere. Each take had one discontinuity flag, on the first packet, and no silent or timestamp-error packets.

The processing does delay the content:

| Measure | Unprocessed | Speech processing | Added |
|---|---|---|---|
| Cable loop, cross-correlation lag (C-S) | 55.6 ms (corr 0.97) | 186.2 ms (corr 0.93) | **+130.6 ms** |
| QuadCast, burst-envelope offset in the take (Q-S2, offline) | 1.08 s | 1.19 s | **about +110 ms** (±10 ms envelope resolution plus play-start jitter) |
| QuadCast, probe cross-correlation of run 2 (Q-S1, Q-S2) | | 132.1 / 133.6 ms | (normalised correlation 0.10 or less: weak) |

The acoustic AEC-off lag on the QuadCast is 2.7, 4.8 and 3.4 ms (Q-S1, Q-S2, Q-C2). Those peaks are weak, with normalised correlation 0.11 or less, because band-limited noise loses coherence in the room. Plan 15 section 2 measured 3.3 ms on real speech.

### 1.6 Measurements

#### 1.6.1 Digital loop: VB-Audio cable (C-S)

Mic is `CABLE Output`. The reference plays on `Speakers (2- VB-Audio Virtual Cable)`, the cable's render side, which is also the AEC reference. Category Speech. The echo path is linear and exact: what the mic receives is the reference.

| Run | Mic during playback | Outside | Notes |
|---|---|---|---|
| 1, AEC off | -34.3 dBFS | -96.7 | lag 55.6 ms, corr 0.97 |
| 2, Speech (AEC, NS, AGC on; reference set: ok) | **-22.7 dBFS** | -96.7 | output vs reference: lag 186.2 ms, **corr 0.93** |
| 3, Speech, silence | | -96.7 whole take, peak -90.3 | digital silence stays silent |

- ERLE as asked (run 1 minus run 2): **-11.6 dB**. The processed mic is louder.
- Run 1 minus run 2 per 0.5 s, across the 12 s: -14, -14, -8, -11, -9, -12, -12, -12, -9, -13, -12, -12, -10, -13, -12, -11, -9, -13, -11, -12, -10, -12, -11, -12. There is no trend that a converging canceller would show.
- Bursts over late gaps (offline): 64.9 dB unprocessed; 39.3 dB processed. In the processed take the gaps sit near -60 dBFS where the input was digital silence.

#### 1.6.2 Double talk on the cable loop (C-S, offline)

- The far end plays on Realtek Digital Output, which is the AEC reference. It started -7.3 ms from the mic-side playback.
- The mic side (the cable) carries the far end 60 ms later at -6 dB, plus the near end.
- The mic-side track was aligned in the mic take by cross-correlation: offset 1,250.7 ms, correlation 0.93.
- Gains are joint least-squares fits of the mic output on the far and near components as they entered the cable.

| Segment | Mic out | Far in | Near in | Far gain | Near gain | Remainder after fit |
|---|---|---|---|---|---|---|
| Far only, before (2 to 4 s) | -23.6 dBFS | -34.2 | – | +10.0 dB | – | -32.4 |
| Double talk (4.5 to 8 s) | -16.2 | -34.2 | -28.2 | +10.6 dB | +10.7 dB | -27.4 |
| Far only, after (8.5 to 12 s) | -24.7 | -34.2 | – | +9.1 dB | – | -35.3 |
| Near only (13.5 to 17 s) | -21.4 | – | -28.2 | – | +6.5 dB | -33.1 |

#### 1.6.3 Run-time control (C-S, Q-S1, Q-S2 run 4)

| Event on the running Speech stream | Result | Cable mic level, 0.5 s after (C-S) | QuadCast burst contrast (Q-S2) |
|---|---|---|---|
| Reference set before Start (the playback endpoint) | ok | -22.1 dBFS (corr 0.94) | 13.2 dB |
| Reference set to another endpoint (Realtek for C-S, which was silent; Speakers (HyperX QuadCast) for Q-S1 and Q-S2) | **S_OK** | -24.3 (corr 0.80) | 15.9 dB |
| Reference set back to the playback endpoint | **S_OK** | -24.1 (corr 0.80) | 15.2 dB |
| `SetAudioEffectState(AEC, OFF)` | **0x88890042, AUDCLNT_E_EFFECT_STATE_READ_ONLY**; effects still AEC, NS, AGC on | -22.6 (corr 0.91) | 9.2 dB |

Nothing in these levels follows the reference. That is consistent with 1.6.1: no cancellation in any segment.

#### 1.6.4 QuadCast and speakers (acoustic)

Mic is the QuadCast, unmuted. The reference plays on Realtek Digital Output, which is the AEC reference. The owner was present, so the room was not silent. Two unprocessed takes a minute apart gave burst contrasts of 4.2 and 13.0 dB, so this metric varies by about 9 dB between takes here.

| Invocation, run | Processing | Mic during playback | Outside | Burst contrast (lag) | Silent take |
|---|---|---|---|---|---|
| Q-S1, run 1 | none | -38.2 dBFS | -47.6 | – | |
| Q-S1, run 2 | Speech: AEC, NS, AGC | -23.8 | -24.3 | – | |
| Q-S1, run 3 | Speech | | | | -22.3 dBFS; peak -9.4; loudest 100 ms -19.9 |
| Q-S2, run 1 | none | -38.2 | -47.6 | 5.0 dB (-20 ms, the search edge) | |
| Q-S2, run 2 | Speech: AEC, NS, AGC | -23.7 | -24.3 | **15.2 dB** (100 ms) | |
| Q-S2, run 3 | Speech | | | | **-20.0 dBFS; peak -3.9**; loudest 100 ms -16.5 |
| Q-C2, run 1 | none | -35.9 | -47.6 | 4.2 dB (5 ms) | |
| Q-C2, run 2 | Communications: no effects listed | -38.3 | -44.5 | 13.0 dB (5 ms) | |
| Q-C2, run 3 | Communications | | | | -42.3 dBFS; peak -27.6; loudest 100 ms -37.2 |

- ERLE as asked (run 1 minus run 2 during playback):
  - Speech: -14.3 dB (Q-S1) and -14.5 dB (Q-S2).
  - Communications: +2.4 dB (Q-C2). That stream had no processing, so this is take-to-take variation.
- Bursts over late gaps for Q-S2 (offline envelope alignment): 3.3 dB unprocessed, 14.3 dB processed.
- Echo above the room floor in the unprocessed takes: 9.4 to 11.8 dB.

#### 1.6.5 Ducking

| Invocation | Mic opened | Tone on | Loopback baseline | Change while open, for each of none / Communications / Speech / GameChat | Change after close |
|---|---|---|---|---|---|
| D1 (with the QuadCast muted; mic state does not matter here) | QuadCast (not default comms) | Realtek Digital Output | -54.7 dBFS | 0.0 / 0.0 / 0.0 / 0.0 dB | 0.0 dB |
| D2 (C-S) | CABLE Output (**default communications capture**) | Realtek Digital Output | -36.4 dBFS | 0.0 / 0.0 / 0.0 / 0.0 dB | 0.0 dB |

After the tone's first 0.25 s (its fade-in), the loopback at 0.25 s resolution stayed at the baseline, ±0.5 dB, in both runs. The baseline differs between D1 and D2 because the owner's output volume changed between the two runs; the volume during D1 was not recorded. The AEC reference for the Speech phase was set (ok). For GameChat and Communications there was no control.

#### 1.6.6 Discarded runs

- First run, default devices: the mic resolved to CABLE Output (the default communications capture endpoint), which carried nothing. -96.7 dBFS during and outside playback, so API support only.
- QuadCast with Communications, before it was unmuted: -92.5 dBFS during playback against -77.1 outside, with a flat -92 dBFS floor and a peak of -56 dBFS. The owner confirmed the QuadCast's hardware mute was on. That is the same floor as the silent-mic meetings 4 to 11 in plan 15. Only its ducking part (D1) is used.

### 1.7 Commands

Every invocation ran from the worktree root. `<scratch>` is the session scratchpad. The WAVs and `report.txt` stayed there and are not committed.

| Id | Command |
|---|---|
| (default) | `cargo run -p diarist-cli -- aec-probe --out <scratch>\aec-probe` |
| D1 | `cargo run -p diarist-cli -- aec-probe --out <scratch>\aec-probe\quadcast-communications --mic "HyperX QuadCast" --ducking` |
| C-S, D2 | `cargo run -p diarist-cli -- aec-probe --out <scratch>\aec-probe\cable-speech --mic "CABLE Output" --render "Speakers (2- VB-Audio Virtual Cable)" --category speech --ducking --double-talk` |
| Q-S1 | `cargo run -p diarist-cli -- aec-probe --out <scratch>\aec-probe\quadcast-speech --mic "HyperX QuadCast" --category speech` |
| Q-S2 | `cargo run -p diarist-cli -- aec-probe --out <scratch>\aec-probe\quadcast-speech-2 --mic "HyperX QuadCast" --category speech` |
| Q-C2 | `cargo run -p diarist-cli -- aec-probe --out <scratch>\aec-probe\quadcast-communications-2 --mic "HyperX QuadCast"` |

Each folder holds:
- `ref.wav`, `mic-aec-off.wav`, `mic-aec-on.wav`, `mic-aec-on-silence.wav` and `report.txt`;
- `mic-aec-controls.wav` (run 4);
- `dt-far.wav`, `dt-mic-side.wav`, `dt-near.wav` and `mic-double-talk.wav` (double talk);
- `tone.wav` and `loopback-ducking.wav` (ducking).
