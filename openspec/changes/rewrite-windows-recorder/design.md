# Design

## Context

The screenshot shows Opus, a selected Discord process, separate-file recording, and clipped controls. Upstream commit `a7fdb4e` gives two visible controls identical bounds origins: x=120, y=248. The Opus selector also intersects the checkbox. The resize handler moves the checkbox and format dropdown but omits the codec-specific controls. Window scaling can compound the overlap. [Resize handler](https://github.com/masonasons/AudioCapture/blob/a7fdb4e822bde95e462827cccca55fb008c0c5ae/src/main.cpp#L518-L565) · [Control creation](https://github.com/masonasons/AudioCapture/blob/a7fdb4e822bde95e462827cccca55fb008c0c5ae/src/main.cpp#L703-L774)

## Goals

Preserve the user's existing workflow. Produce intelligible, synchronized audio for later transcription. Keep the capture path local. Start with the user's Windows 11 x64 configuration.

## Decisions

1. **Layout.** Measure controls with the active font and monitor DPI. Put source filters, codec settings, gains, and transport in separate rows. Wrap or scroll the settings panel when space runs out. Use the same layout pass on creation, resize, DPI changes, and conditional visibility changes.
2. **Units.** Keep codec bitrate in kbps. Show the effective recording sample rate in Hz. Propose Auto, 16000, 44100, and 48000 Hz for PCM WAV. Other codecs may constrain the rate. Expose only supported values and show the effective rate. Treat a WAV rate selector as new behavior.
3. **Source routing.** Use Windows WASAPI process-tree loopback for a selected PID, endpoint loopback for explicit system capture, and capture endpoints for microphones. Keep devices and process identities distinct. Report activation failure without widening the audio scope. Microsoft documents build 20348 as the process-loopback minimum. [Microsoft sample](https://learn.microsoft.com/en-us/samples/microsoft/windows-classic-samples/applicationloopbackaudio-sample/)
4. **Recording engine.** Keep disk and encoder work outside the audio callback. Use bounded queues, timestamped packets, a shared timeline, and explicit overrun reporting. Resample inputs once per destination format. Apply source gain before mixing. Keep a shared silence mask for synchronized tracks when silence skipping is enabled.
5. **Deployment.** Prefer a native Rust UI with accessible Windows controls. Evaluate redistributable codec bindings and system APIs against a portable x64 build. Report executable size, installed dependencies, idle memory, and CPU during capture against the same upstream workload before claiming a footprint improvement.
6. **Settings.** Read legacy JSON settings and named presets into a versioned Rust configuration. Resolve current device and process identities at load time. Keep original files intact.
7. **Rig.** Rig provides Rust interfaces to model providers and agents. Reconsider it for an optional transcript-summary feature. The recorder needs Windows audio, encoding, and UI libraries. [Rig project](https://github.com/0xplaygrounds/rig)
8. **Provenance.** Write the implementation from observable behavior and platform documentation. The upstream README specifies personal and educational use. The author declined a standard license. Keep upstream notices and evaluate redistribution rights for each new codec dependency. [Upstream terms](https://github.com/masonasons/AudioCapture#license) · [Author response](https://github.com/masonasons/AudioCapture/issues/7)

## Verification and rollout

Use generated tones from two processes and a synthetic input for automation. Capture a separate Windows device result for microphone and monitor routing. Compare decoded files, timestamps, and frequency content. Verify the clipped row with control bounds, screenshots, and keyboard interaction before and after resize. Leave the user's running recorder and current recording files untouched during testing.

## Decisions for review

The proposed first target is Windows 11 x64. Older Windows system-only compatibility needs a separate support decision. The proposed adjustable WAV rates go beyond the upstream selector. External transcription is the first delivery workflow. The UI toolkit and codec libraries remain implementation choices.
