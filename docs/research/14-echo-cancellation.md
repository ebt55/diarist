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

## 2. App-side canceller spike (20 Oct 2026)

Plan 15 chunk B1: can a linear echo canceller written in Rust clean the mic channel of meeting 12 enough to matter? The canceller is `crates/core/src/audio/aec.rs` (library module and unit tests). The evaluation is `crates/core/tests/aec_spike.rs`, gated on `DIARIST_ECHO_SAMPLE` / `DIARIST_ECHO_DB` / `DIARIST_ECHO_OUT`. Nothing is wired into the live pipeline or the post-pass. Opus 5.5 implementation agent. Scratch outputs (WAVs, transcripts, `report.txt`, the analysis scripts) stay in the session scratchpad; no transcript text is quoted here.

**Verdict: yes, the canceller makes the owner's double-talk words recognisable, and it removes 12 dB of echo on real audio (15 dB median per word; 14.7 dB with residual suppression; 10.7 dB through the clock anchors alone).** With the owner's own recorded voice spliced over the meeting's real echo, the owner's word error rate falls from 17.1 to 6.7 percent. Over the whole meeting it removes 70 percent of the echo duplicates in "me" (94.5 percent with residual suppression). It gets there only with the reference re-timed from the signals: the clock anchors leave a 5 ms sawtooth and a 60 µs wobble that come from the playback or capture hardware. With the post-AGC loopback as reference it also adds energy to the owner's double-talk words.

**Summary** (numbers in 2.2 to 2.7):

- **Echo removed, far end only** (the duplicates' spans, 100 Hz high-passed):
  - Linear filter: 12.2 dB energy-weighted, 15.1 dB median per word.
  - With residual suppression: 14.7 dB and 19.9 dB.
  - The mic's noise floor caps what can be measured at about 20 dB.
  - Below 1 kHz: 13 dB (16 dB with suppression). At 1 to 2.5 kHz: 7.3 dB (9.5 dB).
- **Owner alone** (loopback silent): level −0.12 dB.
  - Log-spectral distance 0.88 dB with the post-AGC reference, 0.15 dB with the AGC undone.
- **Double talk:**
  - Spliced ground truth: the owner over the echo goes from +8.9 to +19.5 dB, and the owner's level changes by +0.04 dB.
  - The 172 labelled double-talk words: +0.27 dB with the post-AGC reference. The canceller adds energy there, because the AGC raised the reference while the owner talked over a quiet far end. With the AGC undone: −0.22 dB, and the adaptive filter matches a filter frozen over those words to 0.01 dB.
  - No divergence over 111 minutes (0 watchdog resets).
- **Re-transcription** (the repo's `diarist-cli transcribe`; the mic.wav baseline reproduces the stored words 99.5 percent):
  - Duplicates: 12,264 → 3,687 (linear) → 676 (with suppression).
  - All echo words in "me" (duplicates plus garbled non-duplicates outside the owner's stretches): 13,162 → 5,274 → 1,117.
  - Labelled owner's words kept: 1,417 of 1,462. Of the 45 missing, 17 are one repeated-token loop of the raw transcript and 10 are spelling variants; 18 (1.2 percent) are gone.
- **Alignment:**
  - Under the anchors, the echo lag slides 8.8 ppm and jumps back about 5 ms every 9.45 minutes (11 times). It also wobbles 40 to 85 µs RMS from second to second.
  - Re-timing the reference from the signals is worth 1.5 dB overall and 2.4 dB at 400 to 1,000 Hz.
  - Whole-sample against fractional placement, and filter length (130 to 260 ms), change nothing.
- **Cost:** 140 µs per 100 ms frame for the 200 ms filter (0.14 percent of one core), 110 µs for 130 ms, 156 µs with suppression.
- **Causal configuration** (what a live canceller would have: clock anchors only, no look-ahead trackers, with suppression):
  - 14.0 dB of echo removed (18.6 dB median per word).
  - Duplicates 12,264 → 1,710 (−86 percent); echo words in "me" 13,162 → 2,625 (−80 percent).
  - Owner's labelled words kept 1,413 (96.6 percent; 98.6 percent counting the loop and the variants as kept); double-talk words kept 164 of 172.
  - Spliced double-talk word error rate 8.3 percent (raw 17.1).
  - With the AGC undone as well (what the post-pass would have once the gain track exists): 14.0 dB; duplicates 1,757; owner's words kept 1,409; double-talk words kept 164; spliced word error rate 7.5 percent.
- **Against the go/no-go bars of plan 15 section 3.3** (alignment as built here, which looks ahead; post-AGC reference):

| Bar | Linear | With residual suppression | Causal: anchors only, with suppression |
|---|---|---|---|
| At least 15 dB of echo removed on far-end-only stretches | 12.2 dB energy-weighted, 15.1 dB median per word: missed as energy | 14.7 dB energy-weighted, 19.9 dB median: missed by 0.3 dB as energy | 14.0 dB, 18.6 dB median: missed by 1.0 dB as energy |
| Owner's own stretches changed by at most 1 dB | −0.12 dB level, 0.88 dB spectral distance: met | −0.12 dB, 1.19 dB: level met | −0.12 dB, 1.10 dB: level met |
| No divergence over 111 minutes | 0 resets, but 34 two-second windows louder than the mic by over 1 dB (0 with the AGC undone) | 0 resets, 3 windows | 0 resets, 3 windows |
| At most 5 percent of a core | 0.14 percent: met | 0.16 percent: met | 0.16 percent: met |
| Owner's labelled words kept, at least 98 percent | 96.9 percent as counted; 98.8 percent counting the loop and the variants as kept | 96.7 percent; 98.6 percent | 96.6 percent; 98.6 percent |
| Duplicates cut by at least 90 percent before the gate | 70 percent: missed | 94.5 percent: met | 86.1 percent: missed |

### 2.1 The canceller

`audio::aec::Aec`:

- **Algorithm.** A multidelay block frequency-domain adaptive filter (MDF; Soo and Pang, IEEE Trans. ASSP 38(2), 1990), with:
  - Valin's per-bin learning rate (IEEE Trans. ASLP 15(3), 2007). It is the estimated residual echo over the error power, from a running regression of the error spectrum on the echo estimate's (the "leak").
  - The two-path model. The background filter adapts all the time. It is copied to the foreground filter, whose output is used, only when measurably better, and reset from it when measurably worse.
  - Proportionate weighting over the partitions, the gradient constraint on partition 0 plus one rotating partition per block, pre-emphasis 0.9, a DC notch on the mic, and a watchdog that resets on runaway output.
- **Source.** The structure and constants follow Speex's `libspeexdsp/mdf.c` (Jean-Marc Valin, BSD 3-clause). The Rust is an independent implementation, named in the module header. FFTs use `realfft` (in the tree through rubato; the direct dependency came with chunk A).
- **API.** `Aec::process(mic, reference, out)` takes any whole number of blocks, as `f32` at full scale 1.0.
- **`AecConfig`:**
  - `block`: a divisor of 1,600 samples; default 160 (10 ms).
  - `filter_len`: default 3,200 samples (200 ms), rounded up to whole blocks.
  - `bulk_delay`: signed; default −80, a 5 ms lookahead. Negative values delay the mic, positive ones the reference. The default covers lags of −5 to +195 ms.
  - `residual`: optional suppression, a Wiener gain per bin against leak × the echo estimate's spectrum, held over the reverberation, floored at −12 dB. It works by overlap-add of two-block square-root Hann frames.
- **Also exposed:** `set_adaptation(false)` freezes the filter (for the oracle in 2.5), `stats()` returns resets, foreground and background events and the leak, `latency()` the delay, `Notch`, and `SincInterpolator` (32-tap Kaiser windowed sinc, 512 phases, passband to 7.6 kHz).
- **Live cost of the alignment and latency:**
  - The output lags the mic by the lookahead (5 ms by default), plus one block (10 ms) with residual suppression.
  - A mic frame can be cancelled only once the loopback has delivered the same span plus the lookahead, so the mic waits for the later of the two channels.
  - Block processing adds nothing for a caller already working in 100 ms frames.

### 2.2 Synthetic tests (`cargo test -p diarist-core --lib audio::aec`)

Setup:

- Reference: speech-like noise bursts (two moving resonances, 120 to 400 ms on, 60 to 300 ms off) at −26 dBFS.
- Echo: through a synthetic room (3 ms direct path, early reflections, 200 ms RT60 tail to 120 ms) at −10 dB.
- Near end: independent bursts 5 dB above the echo.
- Floor: −70 dBFS.
- Default configuration, release build.

| Test | Result | Assertion |
|---|---|---|
| Far end only, 12 s | ERLE 29.8 dB after 3 s (9.5 dB in the first second; the floor caps at 32.1 dB) | at least 20 dB |
| Near end only after convergence, without and with residual suppression | level −0.33 dB (the DC notch on the bursts' low end), log-spectral distance 0.24 dB | within 1 dB |
| 60 s alternating far end only, double talk, near end only, double talk (3 s each), owner 5 dB over the echo | 0 resets; far-end-only ERLE after each double-talk stretch 30.1, 29.3, 31.0, 32.5 dB; echo removed under the double talk 31.7 dB (exact, from the decomposition); output against mic in double talk −1.47 dB; 37 background resets (the two-path safeguard catching the disturbed background) | 0 resets; at least 15 dB after each; at least 10 dB under double talk; never louder than the mic |
| Reference clock −50 ppm against the mic (the echo slides 0.8 samples a second) | uncorrected 10 to 11 dB in every 10 s (10.7 dB at 40 to 60 s): the filter does not keep up. With the caller's correction (`SincInterpolator` at the known ratio): 32.6 dB | corrected at least 20 dB and better than uncorrected |
| Benchmark (one thread, this PC, release) | 110 µs per 100 ms frame (130 ms filter), 140 µs (200 ms), 167 µs (260 ms), 156 µs (200 ms with suppression) | printed |

The synthetic echo is exactly linear and time-invariant, so these numbers are the filter's own ceiling, not a forecast for a room.

### 2.3 Alignment on meeting 12

- **Timeline.** The mic is processed on its own sample clock and never resampled, so `mic-aec.wav` is sample-aligned with `mic.wav` and maps to meeting time through the same clock map. The reference is placed on that clock: mic sample, then QPC through the mic's anchors, then the loopback sample through the loopback's (fractional), read with `SincInterpolator`. Over 111 minutes the loopback index runs 343.6 ms behind the mic index (−51.6 ppm).
- **Anchors.**
  - The loopback anchors are an exact line: residual 0, slope 625.0000 × 100 ns per sample.
  - The mic's regular anchors jitter about 13 to 20 µs.
  - Every 234 s the mic has a gap pair (28 in the meeting). One anchor of the pair is displaced about ±600 µs, so the piecewise map carries a ±0.6 ms bump for about 2 s. The offset after a pair steps by about ±0.2 ms and reverts at the next pair.
- **The lag under the anchors is a sawtooth, not flat** (GCC-PHAT per 2 s window, 4,695 active windows):
  - Median 2.71 ms, p5 to p95 0.44 to 5.2 ms. It slides at −8.75 ppm (p10 to p90 −9.30 to −8.07), then jumps back on a 9.45-minute period.
  - The jumps are 74 to 79 samples (about 5 ms) at 9.4, 18.9, 37.9, 47.4, 56.8, 85.2, 94.7 and 104.2 minutes. They are smaller (+59 and +55) at 28.3 and 75.7 minutes, where the owner talks most.
  - In the sparse stretch 60 to 68 minutes the detector found −53 at 60.7 and +37 at 66.8 minutes instead of the expected jump near 66.2.
  - No anchor shows the jumps, so a component in the playback or capture hardware slips 5 ms. One candidate is a speaker or receiver running on its own clock.
  - Plan 15 section 2's "median 3.3 ms, p10 1.3, p90 5.5, flat over 111 minutes" is this sawtooth seen through 5-minute medians.
- **Stage 2, lag track.** A line per segment between detected jumps (residual 4.3 samples, the estimator's), and the reference re-placed so that the lag stays at 2.5 ms. Afterwards: p5 to p95 2.1 to 3.4 ms.
- **Stage 3, fine wobble.** Per 1 s window, hop 0.5 s: the slope of the cross-spectrum phase over 300 Hz to 3 kHz against the ±15 s average. Before: 85 µs RMS, p95 step 212 µs. After correction: 41 µs, p95 step 106 µs, about the estimator's noise.
  - Offline checks on 3-minute stretches: the same wobble from 300 to 1,200 Hz (62 µs) and from 1,200 to 3,000 Hz (41 µs). Smoothing the mic's anchors removes only 7 to 11 µs of it, so most of it is physical.
  - Timing jitter of 60 µs alone caps a linear canceller near 8.5 dB at 1 kHz and 2.5 dB at 2 kHz.
- **Both trackers look ahead** (centred windows, ±15 s average). They are the offline best case. A causal live version would trail each change by about a window and has not been built.
- **AGC.** The stored loopback is after the app's AGC.
  - Replayed backwards (the gain law of `audio::gain` at default settings, 10 ms blocks assumed aligned to the stream), the gain is above 0.1 dB in 30.8 percent of blocks, above 6 dB in 19.5 percent and above 20 dB in 14.6 percent (median of the non-zero blocks 15.7 dB).
  - Most far-end speech peaks above the −20 dBFS target, so its gain is 0 dB. The AGC raises the quiet far end, the room tone while the owner talks, and quiet talkers.
  - The "AGC undone" rows use that replay as the reference. It stands in for a pre-AGC tap.

### 2.4 Echo removed on meeting 12 (signal)

Spans:

- Far end only: the 11,590 duplicate words with the loopback active.
- Owner alone: the 1,290 owner's words with the loopback silent.
- Double talk: the 172 owner's words with the loopback active.

All signals are 100 Hz high-passed. The word classes are recomputed in the test from the stored word files and the anchors, and reproduce the measurement's counts exactly: 12,260 duplicates, 1,462 owner's words in 27 runs, 172 during far-end speech, 748 misrecognised echo.

| Variant (reference alignment, filter) | ERLE, far end only | per word p10 / p50 / p90 | Owner alone: level | spectral distance | Double talk: level | 2 s windows louder than the mic | Resets |
|---|---|---|---|---|---|---|---|
| Anchors, 200 ms | 10.7 dB | 4.0 / 13.7 / 20.5 | −0.12 dB | 0.79 dB | +0.31 dB | 35 | 0 |
| Anchors, 200 ms + residual (causal; `mic-aec-anchor-res.wav`) | 14.0 dB | 8.4 / 18.6 / 26.7 | −0.12 dB | 1.10 dB | −0.47 dB | 3 | 0 |
| Anchors, whole-sample reference | 10.6 dB | 3.9 / 13.3 / 19.8 | −0.12 dB | 0.76 dB | +0.30 dB | 34 | 0 |
| Anchors, AGC undone | 10.9 dB | 4.1 / 13.8 / 20.6 | −0.12 dB | 0.15 dB | −0.20 dB | 0 | 0 |
| Lag track, 200 ms | 11.1 dB | 4.4 / 14.4 / 21.6 | −0.12 dB | 0.89 dB | +0.25 dB | 32 | 0 |
| **Lag track + fine, 200 ms** (`mic-aec.wav`) | **12.2 dB** | 5.9 / 15.1 / 21.9 | −0.12 dB | 0.88 dB | +0.27 dB | 34 | 0 |
| **Lag track + fine, 200 ms + residual** (`mic-aec-res.wav`) | **14.7 dB** | 9.3 / 19.9 / 28.2 | −0.12 dB | 1.19 dB | −0.47 dB | 3 | 0 |
| Lag track + fine, 130 ms | 12.2 dB | 5.8 / 14.4 / 20.9 | −0.12 dB | 0.86 dB | +0.15 dB | 30 | 0 |
| Lag track + fine, 260 ms | 12.2 dB | 5.7 / 14.9 / 22.0 | −0.12 dB | 0.88 dB | +0.24 dB | 33 | 0 |
| Lag track + fine, 20 ms blocks | 11.8 dB | 5.7 / 15.0 / 21.9 | −0.12 dB | 0.90 dB | +0.70 dB | 41 | 0 |
| Lag track + fine, AGC undone (`mic-aec-raw.wav`) | 12.5 dB | 6.1 / 15.2 / 22.0 | −0.12 dB | 0.15 dB | −0.22 dB | 0 | 0 |
| Lag track + fine, AGC undone + residual | 14.7 dB | 9.3 / 19.9 / 28.2 | −0.12 dB | 0.57 dB | −0.37 dB | 0 | 0 |
| Anchors, AGC undone + residual (causal, post-pass with the gain track; `mic-aec-anchor-raw-res.wav`) | 14.0 dB | 8.4 / 18.6 / 26.7 | −0.12 dB | 0.57 dB | −0.37 dB | 0 | 0 |

- The measurement ceiling (the echo at the mic over the mic noise floor, on the same spans) is 20.4 dB.
- The owner-alone change of −0.12 dB is the DC notch. The spectral distance there is the echo estimate subtracted from the AGC-raised far-end room tone, which goes away with the AGC undone.

Per band, far end only:

| Variant | 100–400 Hz | 400–1,000 Hz | 1–2.5 kHz | 2.5–8 kHz |
|---|---|---|---|---|
| Anchors, 200 ms | 12.2 dB | 10.6 dB | 5.8 dB | 1.7 dB |
| Lag track, 200 ms | 12.5 dB | 10.9 dB | 6.4 dB | 3.0 dB |
| Lag track + fine, 200 ms | 13.2 dB | 13.0 dB | 7.3 dB | 3.3 dB |
| Lag track + fine, 200 ms + residual | 15.5 dB | 16.5 dB | 9.5 dB | 4.6 dB |
| Lag track + fine, AGC undone | 13.6 dB | 13.2 dB | 7.4 dB | 3.3 dB |
| Anchors, 200 ms + residual (causal) | 15.3 dB | 15.3 dB | 8.3 dB | 2.8 dB |
| Anchors, AGC undone + residual (causal) | 15.5 dB | 15.3 dB | 8.2 dB | 2.8 dB |

- **Where the residual sits** (offline, 34 to 44 min, far end only, on the "lag track, 200 ms" output, against the mic floor measured in 526 quiet 2 s windows):
  - At 100 to 400 Hz the output is within 1 to 2 dB of the floor: the canceller takes all the echo there.
  - At 400 to 700 Hz it sits 4.7 dB over the floor, and at 0.7 to 2.5 kHz 6 to 7 dB over.
  - A per-100 ms gain correction of the echo estimate gains only 0.4 dB (13.4 to 13.8 dB), so the residual is not a level error such as a compressor after the tap.
  - On 20 s chunks a fixed least-squares filter (coherence bound) reaches about 10 dB where the adaptive filter reaches 14.5 dB. The echo path changes within seconds, consistent with the timing wobble.

Per 10-minute bucket ("lag track + fine, 200 ms"; duplicates in the bucket in brackets):

- 0 min: 13.3 dB (644); 10: 14.8 (1,293); 20: 7.3 (779); 30: 9.2 (1,393); 40: 12.9 (1,390); 50: 16.4 (1,192).
- 60 min: 15.5 dB (204); 70: 9.0 (996); 80: 11.7 (1,073); 90: 13.1 (1,360); 100: 16.2 (1,254); 110: 12.1 (18).
- No trend over the meeting. The low buckets (20 to 40 and 70 to 80 min) are where the owner talks most. That stays the same with the AGC undone (7.4 and 9.2 dB) and is not explained.

The word gate's statistic on the cancelled signal (duplicates' spans, plan 15 section 3.2):

- Raw mic: NCC median 0.74.
- After the linear canceller: 0.17, with 14.7 percent at or above 0.35.
- With suppression: 0.09 and 6.1 percent.
- The owner's double-talk words stay at 0.06.

### 2.5 Double talk

**Oracle on the 172 real double-talk words.** The canceller is rerun with adaptation frozen over each word ±0.3 s, so its estimate there is the one learnt on the far end alone.

| Reference | Frozen echo estimate against the mic | Frozen output against the mic | Adaptive output against frozen |
|---|---|---|---|
| Post-AGC (lag track + fine) | −1.9 dB | +1.66 dB | −1.39 dB |
| AGC undone | −13.8 dB | −0.21 dB | −0.01 dB |

With the AGC undone, the echo under those words is about 14 dB below the owner. The canceller removes it without touching the owner, and the two-path safeguard keeps the adaptive filter identical to the frozen one. With the post-AGC reference, the learnt path over-predicts by the AGC's gain, so the canceller adds energy exactly where the owner talks over a far end that has just resumed.

**Spliced double talk with ground truth:**

- All 116 owner-alone utterances (251 s, the owner's own voice from the loopback-silent stretches) are placed one after another, 1.5 s apart, on the far-end-only window 34 to 44 minutes (1,730 duplicate words, no owner's word). 73 percent of the owner's samples fall on active far-end frames.
- The canceller runs from a minute before the window. Because `out = notch(echo + owner) − estimate`, `out − notch(owner)` is the residual echo exactly.

| Variant | Owner over echo, before | after | Echo removed under the owner | Echo removed, rest of the window | Output against the owner alone |
|---|---|---|---|---|---|
| Lag track + fine, 200 ms | +8.9 dB | +19.5 dB | 10.6 dB | 12.6 dB | +0.04 dB |
| + residual | +8.9 dB | +18.4 dB | 9.4 dB | 16.4 dB | −0.20 dB |
| AGC undone | +8.9 dB | +19.5 dB | 10.6 dB | 12.8 dB | +0.04 dB |
| Anchors, 200 ms + residual (causal) | +8.9 dB | +18.0 dB | 9.1 dB | 15.8 dB | −0.20 dB |
| Anchors, AGC undone + residual (causal) | +8.9 dB | +18.0 dB | 9.1 dB | 15.8 dB | −0.20 dB |

The window's far end talks almost continuously, so its AGC gain is near 0 dB and the AGC rows match. The spliced test does not show the AGC's double-talk problem; the oracle above does.

### 2.6 Re-transcription

- **Engine and flags.** `diarist-cli transcribe <wav> --no-diarize --json`, built from this worktree (`cargo build --release -p diarist-cli`). It used a scratch data folder (not the owner's database), the installed engine (nemo-speech 0.2.0, CUDA) and models (Nemotron EN 0.6B q8_0, lookahead 13), and the file settings.
- **Wall time.** 43 to 84 s per 111-minute file, 4 to 5 s per 10-minute file, well inside the 20-minute budget.
- **Mapping and scoring.** Word times are mapped to meeting time through the mic's clock map, as the post-pass does, and scored with the measurement's matcher against `loopback.words.json`.
- **Baseline.** `mic.wav` gives 14,628 words against the stored 14,627; 99.5 percent are in the stored file with the same text, starting within 50 ms. So the baseline reproduces the stored transcript.

| Transcript | Words | Duplicates | Non-duplicates outside the owner's stretches (garbled echo) | Words in the owner's stretches | Labelled owner's words kept (of 1,462) | New non-duplicates there | Double-talk words kept (of 172) | Words in the double-talk words' spans |
|---|---|---|---|---|---|---|---|---|
| mic (raw) | 14,628 | 12,264 (83.8 %) | 898 | 1,467 | 1,461 | 2 | 172 | 184 |
| mic-aec (linear) | 6,697 | 3,687 (55.1 %) | 1,587 | 1,424 | 1,417 | 19 | 165 | 172 |
| mic-aec-res | 2,537 | 676 (26.6 %) | 441 | 1,421 | 1,414 | 23 | 165 | 175 |
| mic-aec-raw (AGC undone) | 6,424 | 3,490 (54.3 %) | 1,512 | 1,423 | 1,418 | 19 | 165 | 174 |
| mic-aec-raw-res | 2,451 | 588 (24.0 %) | 438 | 1,426 | 1,416 | 23 | 165 | 173 |
| mic-aec-anchor-res (causal) | 4,046 | 1,710 (42.3 %) | 915 | 1,421 | 1,413 | 21 | 164 | 172 |
| mic-aec-anchor-raw-res (causal, AGC undone) | 4,114 | 1,757 (42.7 %) | 941 | 1,416 | 1,409 | 24 | 164 | 170 |

- **Echo words left in "me"** (duplicates plus garbled): 13,162 raw, 5,274 linear (−60 percent), 1,117 with suppression (−92 percent); 5,002 and 1,026 with the AGC undone. The causal configurations leave 2,625 (anchors only with suppression, −80 percent) and 2,698 (the same with the AGC undone). The linear canceller turns part of the echo into quieter, garbled words: 898 → 1,587. Suppression removes those as well.
- **Missing labelled owner's words** (offline):
  - Linear (45 missing): 17 are a single repeated-token loop the raw ASR emitted at one instant, 10 are spelling variants of a word still present (a word merged or split), and 18 are gone (1.2 percent).
  - With suppression (48): 17, 11 and 20.
  - Causal, anchors only with suppression (49): 17, 12 and 20. With the AGC undone as well (53): 17, 12 and 24.
- **On the real recording** the labelled double-talk words were by definition already recognised in the raw mic, so they cannot show recovery: 165 of 172 stay. The spliced test can.

| Spliced double talk (reference: `dt-near.wav` transcribed alone, 630 owner's words in the clips) | Owner's word error rate | Owner's words matched | Echo duplicates inside the owner's clips | Duplicates outside the clips | Non-duplicates outside the clips |
|---|---|---|---|---|---|
| dt-mix (raw) | 17.1 % | 556 | 33 | 533 | 27 |
| dt-mix-aec (linear) | **6.7 %** | 604 | 23 | 131 | 50 |
| dt-mix-aec-res | 7.0 % | 599 | 25 | 8 | 19 |
| dt-mix-aec-raw (AGC undone) | 6.8 % | 602 | 23 | 132 | 61 |
| dt-mix-aec-anchor-res (causal) | 8.3 % | 591 | 25 | 52 | 34 |
| dt-mix-aec-anchor-raw-res (causal, AGC undone) | 7.5 % | 597 | 25 | 48 | 30 |

### 2.7 Cost

- **Unit-test benchmark** (one thread, release, this PC, two runs): 109 to 113 µs per 100 ms frame with the 130 ms filter, 140 to 143 µs with 200 ms, 166 to 169 µs with 260 ms, and 156 µs with 200 ms plus suppression. That is 0.11 to 0.17 percent of one core.
- **On meeting 12** (four variants in parallel on a loaded machine): 100 to 250 µs per frame. 20 ms blocks cost 100 to 150 µs but are worse under double talk (+0.70 dB, 41 louder windows).
- **The whole evaluation** runs in about 100 s: decode 2 s, reference placement 4 s per variant, trackers 2 s, twelve canceller variants over 111 minutes in 41 to 59 s, four at a time.

### 2.8 Open problems

- **The AGC'd reference.**
  - `loopback.wav` is after the AGC. Against it, the canceller makes the output louder than the mic in 34 two-second windows, adds 0.27 dB to the owner's double-talk words and colours the owner-alone stretches (0.88 dB spectral distance).
  - The backward replay of the gain law removes all three. It assumes the AGC's 10 ms blocks start at multiples of 160 samples and default settings. A recording made with other AGC settings, or with packets that are not 160 samples, would need its gain track recorded.
- **Clock and playback timing.**
  - The anchors do not see the 5 ms sawtooth or the 40 to 85 µs wobble.
  - The trackers here look ahead. Live, a lag tracker trails each 5 ms jump by about one 2 s block and the wobble by about a second; the filter re-converges after each jump in about 1 to 3 s.
  - Whether the sawtooth period depends on the output device is unknown: one recording, one output.
- **Residual echo.**
  - At 1 to 2.5 kHz the canceller removes only 7.3 dB (9.5 dB with suppression) while the echo there stands 12 to 14 dB over the floor, and that band carries consonants.
  - Below 400 Hz the floor is reached, so more cancellation there cannot be measured.
  - Suppression takes the transcript's echo words from 5,274 to 1,117, at the price of owner-alone colouring (1.19 dB spectral distance; 0.57 dB with the AGC undone) and −0.20 dB on the spliced owner.
- **The word gate on cancelled audio.** Its statistic falls from 0.74 to 0.17 (0.09 with suppression) on the duplicates' spans. The thresholds measured on the raw mic do not carry over to the cancelled signal; on the raw mic the statistic is unchanged.
- **Unexplained.** ERLE is low (7 to 9 dB) in the buckets where the owner talks most, with and without the AGC.
- **Not covered.** One recording, one room, one output and one mic. No headphone or Bluetooth case, and no device change mid-recording.

### 2.9 Commands

From the worktree root. `<scratch>` is the session scratchpad. `<data>` is `%LOCALAPPDATA%\app.diarist.desktop` and is opened read-only. `<meeting 12>` is meeting 12's folder under `<data>\audio` (the owner's 19 Oct recording).

```text
set DIARIST_ECHO_SAMPLE=<data>\audio\<meeting 12>
set DIARIST_ECHO_DB=<data>\diarist.db
set DIARIST_ECHO_OUT=<scratch>\aec-spike\out
cargo test --release -p diarist-core --test aec_spike -- --nocapture      (writes the WAVs and report.txt, about 100 s)
cargo build --release -p diarist-cli
diarist-cli --data-dir <scratch>\aec-spike\cli-data --models-dir <data>\models --engine-dir <data>\engine transcribe <out>\<name>.wav --no-diarize --json
    (for mic, mic-aec, mic-aec-res, mic-aec-raw, mic-aec-raw-res, mic-aec-anchor-res, mic-aec-anchor-raw-res,
     dt-near, dt-mix, dt-mix-aec, dt-mix-aec-res, dt-mix-aec-raw, dt-mix-aec-anchor-res, dt-mix-aec-anchor-raw-res;
     copy the printed words_json to <out>\<name>.asr.json)
cargo test --release -p diarist-core --test aec_spike -- --nocapture      (again: scores the transcripts)
cargo test --release -p diarist-core --lib audio::aec -- --nocapture       (synthetic tests and the benchmark)
```

The offline analyses (anchor residuals, coherence, per-band floor, the phase-slope wobble checks, the categories of missing words) are scratchpad Python scripts over the WAVs and the read-only database. Each is marked "(offline)" above.
