'use client'

import { createContext, useCallback, useContext, useEffect, useState } from 'react'
import type { Dispatch, ReactNode, SetStateAction } from 'react'

interface ThemeContextValue {
  theme: string
  setTheme: Dispatch<SetStateAction<string>>
}

const ThemeContext = createContext<ThemeContextValue>({
  theme: 'system',
  setTheme: () => {},
})

export function ThemeProvider({
  children,
}: {
  children: ReactNode
}) {
  const [theme, setThemeState] = useState('system')
  const [initialized, setInitialized] = useState(false)

  const setTheme = useCallback<Dispatch<SetStateAction<string>>>((value) => {
    setThemeState((current) => {
      const next = typeof value === 'function' ? value(current) : value
      try {
        window.localStorage.setItem('theme', next)
      } catch {
        // The active theme still applies when storage is unavailable.
      }
      return next
    })
  }, [])

  useEffect(() => {
    try {
      setThemeState(window.localStorage.getItem('theme') ?? 'system')
    } catch {
      setThemeState('system')
    }
    setInitialized(true)
  }, [])

  useEffect(() => {
    if (!initialized) return
    const root = document.documentElement
    const systemPreference = window.matchMedia('(prefers-color-scheme: dark)')
    const applyTheme = () => {
      const resolved = theme === 'system' ? (systemPreference.matches ? 'dark' : 'light') : theme
      root.classList.remove('light', 'dark')
      root.classList.add(resolved)
      root.style.colorScheme = resolved === 'light' || resolved === 'dark' ? resolved : ''
    }
    const handleStorage = (event: StorageEvent) => {
      if (event.key === 'theme') setThemeState(event.newValue ?? 'system')
    }

    applyTheme()
    systemPreference.addEventListener('change', applyTheme)
    window.addEventListener('storage', handleStorage)
    return () => {
      systemPreference.removeEventListener('change', applyTheme)
      window.removeEventListener('storage', handleStorage)
    }
  }, [initialized, setThemeState, theme])

  return <ThemeContext.Provider value={{ theme, setTheme }}>{children}</ThemeContext.Provider>
}

export function useTheme() {
  return useContext(ThemeContext)
}
