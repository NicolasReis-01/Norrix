export type PowerOnAssetUrl = `/power-on/${string}`

export type PowerOnAnimation =
  | { kind: 'image'; src: PowerOnAssetUrl }
  | { kind: 'video'; src: PowerOnAssetUrl }

export interface PowerOnMedia {
  animation: PowerOnAnimation | null
  logo: PowerOnAssetUrl | null
}

export const POWER_ON_MIN_DURATION_MS = 3500
export const POWER_ON_FADE_DURATION_MS = 450

export const powerOnMedia: PowerOnMedia = {
  animation: {
    kind: 'video',
    src: '/power-on/animation/power-on.webm',
  },
  logo: null,
}
