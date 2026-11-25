# Audio Capture for Windows (C++)

`AudioCapture.exe` records Windows application audio, system audio, and input devices. The original Win32 recorder saves WAV, MP3, Opus, or FLAC. Extract the ZIP. Run the executable. The package includes the codecs and C++ runtime in the executable. MP3 uses Windows Media Foundation.

Use Windows 11 x64 for per-process capture. The program has a system-audio mode for older Windows builds. It shows the process list from Windows 10 build 19041. Microsoft's application loopback sample requires build 20348 or later. [Microsoft application loopback sample](https://learn.microsoft.com/en-us/samples/microsoft/windows-classic-samples/applicationloopbackaudio-sample/)

## Record

1. Select one process or check multiple processes. Select **[System Audio - All Processes]** to capture system audio.
2. Select a file format. Adjust bitrate for MP3 or Opus, or compression level for FLAC.
3. Select **Capture inputs**. Check the microphones or line-in devices you want to record.
4. Choose **Separate files**, **Combined file**, or **Both** for multiple sources.
5. Select the output folder. The default is `Documents\AudioCaptures`.
6. Select **Start Capture**. Check the **Active Recordings** list for progress.
7. Select a recording. Choose **Stop Capture**. Use **Stop All** to end all active recordings.

**Separate files** saves each application and input device to its own file. **Combined file** mixes the sources into one file. **Both** saves the mix and the separate files. For multiple active recordings, **Pause All** and **Resume All** control all recordings together.

Enable **Skip silence** to omit silent audio. Files use names such as `chrome-2025_10_12-14_30_45.flac`. The recorder saves preferences and supports named presets.

| Format | Options |
| --- | --- |
| WAV | Uncompressed PCM |
| MP3 | 128, 192, 256, or 320 kbps |
| Opus | 64, 96, 128, 192, or 256 kbps in an Ogg container |
| FLAC | Lossless with compression levels 0 through 8 |

## Monitor

Enable **Monitor audio**. Select a playback device. Start capture to send audio to that device. Select **Monitor only (no recording)** to listen without saving a file.

## Keyboard shortcuts

| Keys | Control |
| --- | --- |
| Alt+A | Available processes |
| F5 | Refresh the process list |
| Alt+H | Show only processes with active audio |
| Alt+F | Format for captured files |
| Alt+M | Monitor audio |
| Alt+I | Capture inputs |
| Alt+O | Output folder |
| Alt+B | Browse |
| Alt+S | Start or stop capture |
| Alt+R | Active recordings |
| Alt+P | Preset list |
| Alt+V | Save preset |
| Alt+L | Load preset |
| Alt+D | Delete preset |

Use Tab to move between controls. Use the arrow keys to move through lists. The process list shows window titles when available. System-audio mode hides process controls.

## Troubleshoot capture

Check that the selected application is playing audio. Refresh the process list if you started the application after the recorder. If the target runs with elevated privileges, try running the recorder with the same privileges. Check that Windows Media Foundation is available if MP3 encoding fails.

## License

This project is provided as-is for educational and personal use.

## Credits

The original AudioCapture was written with Claude Code. It uses WASAPI for capture, Win32 for the interface, Media Foundation for MP3, libFLAC for FLAC, libopus and libogg for Opus, and nlohmann-json for settings. The build uses CMake and vcpkg.
