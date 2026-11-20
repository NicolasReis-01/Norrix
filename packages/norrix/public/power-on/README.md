# Power On animation

The Power On screen uses one full-viewport video: `animation/power-on.webm`.
Its public URL is configured in `packages/norrix/lib/power-on-config.ts` and is
served from `/power-on/...` in development and copied by Next.js into
`packages/norrix/out` for the Tauri desktop build.

The WebM contains VP9 video and embedded Opus audio. It fills the screen and
loops while startup is pending. The app does not use a separate logo image or
an MP4 conversion-source file; neither asset is part of this configuration.
If CEF denies audio autoplay, playback is retried muted.

The video fades out after startup is ready and the configured minimum display
duration has elapsed. The fade duration is configured in
`packages/norrix/lib/power-on-config.ts`.
