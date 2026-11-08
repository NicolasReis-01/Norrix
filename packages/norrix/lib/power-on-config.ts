export type PowerOnAssetUrl = `/power-on/${string}`

export type PowerOnAnimation =
  | { kind: 'image'; src: PowerOnAssetUrl }
  | { kind: 'video'; src: PowerOnAssetUrl }

export interface PowerOnMedia {
  animation: PowerOnAnimation | null
  logo: PowerOnAssetUrl | null
}

export const POWER_ON_MAX_DURATION_MS = 9000
export const POWER_ON_FADE_DURATION_MS = 450
export const POWER_ON_HARD_STOP_AT_MS = POWER_ON_MAX_DURATION_MS
export const POWER_ON_FADE_OUT_AT_MS = POWER_ON_MAX_DURATION_MS - POWER_ON_FADE_DURATION_MS

export const powerOnMedia: PowerOnMedia = {
  animation: {
    kind: 'video',
    src: '/power-on/animation/power-on.webm',
  },
  logo: '/power-on/logo/power-on-logo.png',
}
