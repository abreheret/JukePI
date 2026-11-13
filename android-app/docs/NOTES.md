# Development notes

🇫🇷 [Français](NOTES.fr.md)

Lessons learned, testing tips and roadmap of the Android app. The design is in [ARCHITECTURE.md](ARCHITECTURE.md).

## Lessons learned

- **Android 17 blocks the local network by default** for apps targeting API 37: declare *and* request `ACCESS_LOCAL_NETWORK` before any connection (TCP, mDNS, `.local` names). Without it connections simply time out, with no error. (The earlier `NEARBY_WIFI_DEVICES` rule was only Android 16's interim opt-in.)
- **Never shut down the executor given to `NsdManager`**: the system keeps posting cleanup tasks to it, and the app crashes with a `RejectedExecutionException`.
- **SSE needs a client with no read timeout**, otherwise the stream is cut every few seconds.
- **ExoPlayer cannot seek in MP3 files that have no Xing/Info/VBRI header** unless constant-bitrate seeking is enabled. Half of the files of the test library are like that.
- **ExoPlayer's default seek steps are 5 s back / 15 s forward**: set both to 10 s or the "±10 s" buttons lie.
- **`setMediaButtonPreferences` without an explicit slot replaces Previous/Next** in the notification; put skip buttons in `SLOT_OVERFLOW` to keep five buttons.
- **Hardware media keys go to the app that plays audio locally.** With a remote-controlling `MediaSession` they never arrive; with local ExoPlayer playback they do (Bluetooth headset buttons work).
- **Android keeps the media volume per audio output** (speaker, Bluetooth A2DP, headset…): `dumpsys audio` shows each one.
- **After `adb install -r`, force-stop the app before launching it**, otherwise `am start` reports "task brought to the front" and no process exists.
- **`dumpsys media_session` positions are not reliable**: read the times from the UI (`uiautomator dump`).
- **Compose icons**: draw the note head and the stem as separate paths, or opposite winding directions cancel where they overlap.
- A WorkManager download that must outlast ~10 minutes needs a foreground service of type `dataSync` (and its manifest override).

## Testing tips (adb)

- Screenshots: `adb exec-out screencap -p > shot.png`. The phone may show something other than the app (home screen, notifications, messages): look only at what concerns the app.
- Never capture the notification shade or the lock screen: use `adb shell dumpsys notification` filtered on `pkg=app.musicremote`, or `uiautomator dump` and parse only the app's own nodes.
- Drive the UI with `adb shell input tap/swipe/text`; check the state first if someone may also be using the phone.
- Grant permissions: `adb shell pm grant app.musicremote android.permission.ACCESS_LOCAL_NETWORK` (and `POST_NOTIFICATIONS`).
- **`adb shell pm clear` also deletes every downloaded album.**
- Downloaded files: `adb shell ls -l /sdcard/Android/data/app.musicremote/files/music/`.
- Tests that disturb whoever holds the phone (cutting Wi-Fi with `adb shell svc wifi disable`, moving or stopping playback, changing the output, making sound): lower the media volume first and restore the network and the volume afterwards.
- Crashes: `adb logcat -d | grep -E "FATAL|AndroidRuntime"`.

## Roadmap

- [ ] Check the lock screen visually (controls, cover, title) and the notification permission prompt on a fresh install.
- [ ] Test the output hand-over while playing (amplifier → phone and back) and an interrupted download (kill the app mid-download, check the resume).
- [ ] A volume-step setting for the volume keys (2 today) and an optional safety volume cap.
- [ ] A server `mute` toggle is done; a sleep timer and playlists are not.
- [ ] Sort downloaded albums by artist; download several albums at once.
- [ ] Release build with signing, to install without a cable.
