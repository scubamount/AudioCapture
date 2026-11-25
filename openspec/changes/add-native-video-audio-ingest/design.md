## Context

Recognition endpoints accept audio. Some reject MP4 video. Others accept the file and receive images the user never needed to send.

## Goals

Use one import path for an individual video, a mixed batch, and the scripted worker. Preserve the existing consent and queue boundaries.

## Decisions

1. Recognize MP4, M4V, MOV, MKV, WebM, MPEG, MPG, AVI, and WMV by extension. The installed Windows codecs determine which files decode.
2. Use a synchronous Media Foundation source reader in the worker. Open the held input and select its first embedded audio stream.
3. Request 16 kHz, mono, 16-bit PCM. Feed the existing WAV splitter one buffer at a time. Limit each part including its WAV header.
4. Use elapsed decoded audio frames for transcript offsets. An edit or a delayed track can place speech at a different time in video playback.
5. Include the conversion method in the job identity. A cached whole-video upload cannot satisfy a native audio job.
6. Report an error on a missing audio track, an unsupported codec, or a decode failure. Preserve ordinary per-file queue recovery.

Windows provides demuxing, decoding, and resampling. No external media tool or model is required for video import. [Microsoft source reader guide](https://learn.microsoft.com/en-us/windows/win32/medfound/tutorial--decoding-audio)

Windows can enumerate MP4 audio tracks in a different order from the file. The worker scans MP4 box headers to find the first audio track. Its read-only stream hides later audio track headers from Windows. Media payloads keep their original offsets.

The standalone worker loads the Windows media libraries only for video import. If these libraries are missing, video jobs report the missing component.

## Verification

Generate a short MP4 with distinct audio tracks, a long MP4, and a video with no audio. Inspect requests at a loopback test endpoint.
Check the WAV format, decoded signal, frame offsets, part sizes, consent, and cache reuse. Keep playback disabled throughout.
