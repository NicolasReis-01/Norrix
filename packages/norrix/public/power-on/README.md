# Power On media

Place the supplied files in the matching folders below, then set their
project-relative public URLs in `packages/norrix/lib/power-on-config.ts`.
The supplied animation and logo are configured in
`packages/norrix/lib/power-on-config.ts`. Assets are served from `/power-on/...`
in development and copied by Next.js into `packages/norrix/out` for the Tauri
desktop build.

The configured assets are `animation/power-on.webm` and
`logo/power-on-logo.png`. The WebM contains the startup audio; no separate
audio file is used or supported. The original MP4 is retained as the conversion
source and is not used by the app.

- `animation/power-on.webm`: VP9 video with embedded Opus audio. WebM is used
  because the bundled CEF does not support the original H.264/AAC MP4. The
  original `animation/power-on.mp4` is retained unchanged. The WebView plays
  the WebM from the beginning and pauses it at fade-out.
- `logo/power-on-logo.png`: complementary still PNG.

All sources must use a URL under `/power-on/`; absolute file paths and remote
URLs are intentionally unsupported. Video and image are positioned
independently: the video is centered, and the logo is centered horizontally
below it without participating in layout. The screen fades out as soon as the
native runtime and initial project, page, and font queries are ready. There is
no minimum display time. If they are still pending, the Power On fades out at
8.55 seconds and a black screen is shown at the 9-second safety limit until it
is safe to show the main interface. Startup errors also remain on Power On
until that limit, then show black. If CEF denies audio autoplay, the video
retries muted so the animation remains visible.
