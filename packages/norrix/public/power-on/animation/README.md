The app loads `power-on.webm` from this folder as its full-screen startup
animation. It contains VP9 video and embedded Opus audio for compatibility with
the bundled CEF. No MP4 conversion-source file is shipped. Update
`packages/norrix/lib/power-on-config.ts` if the animation path or playback
kind changes.
