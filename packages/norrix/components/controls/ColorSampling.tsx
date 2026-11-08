'use client'

import {
  createContext,
  type ReactNode,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
} from 'react'

import { useNorrixStore, type CanvasTool } from '@/lib/store'

type ApplyColor = (color: string) => void

interface ColorSampling {
  request: (apply: ApplyColor) => void
  complete: (color: string) => boolean
  cancel: () => void
}

const ColorSamplingContext = createContext<ColorSampling | null>(null)

export function ColorSamplingProvider({ children }: { children: ReactNode }) {
  const pending = useRef<ApplyColor | null>(null)
  const previousTool = useRef<CanvasTool>('select')
  const tool = useNorrixStore((state) => state.tool)
  const setTool = useNorrixStore((state) => state.setTool)

  const request = useCallback(
    (apply: ApplyColor) => {
      if (!pending.current) previousTool.current = useNorrixStore.getState().tool
      pending.current = apply
      setTool('color_picker')
    },
    [setTool],
  )

  const cancel = useCallback(() => {
    const active = pending.current !== null
    pending.current = null
    if (active && useNorrixStore.getState().tool === 'color_picker') {
      setTool(previousTool.current)
    }
  }, [setTool])

  const complete = useCallback(
    (color: string) => {
      const apply = pending.current
      if (!apply) return false
      pending.current = null
      apply(color)
      if (useNorrixStore.getState().tool === 'color_picker') setTool(previousTool.current)
      return true
    },
    [setTool],
  )

  useEffect(() => {
    if (tool !== 'color_picker') cancel()
  }, [cancel, tool])

  const value = useMemo(() => ({ request, complete, cancel }), [cancel, complete, request])
  return <ColorSamplingContext.Provider value={value}>{children}</ColorSamplingContext.Provider>
}

export function useColorSampling(): ColorSampling | null {
  return useContext(ColorSamplingContext)
}
