import type { Metadata } from 'next'
import localFont from 'next/font/local'
import Script from 'next/script'

import './globals.css'
import Providers from '@/app/providers'
import { ThemeProvider } from '@/components/app/ThemeProvider'
import logo from '../../../crates/norrix/icons/icon.png'

const themeInitialization = `(() => {
  try {
    const preference = localStorage.getItem('theme') || 'system'
    const theme = preference === 'system'
      ? (matchMedia('(prefers-color-scheme: dark)').matches ? 'dark' : 'light')
      : preference
    document.documentElement.classList.remove('light', 'dark')
    document.documentElement.classList.add(theme)
    document.documentElement.style.colorScheme = theme === 'light' || theme === 'dark' ? theme : ''
  } catch {
    document.documentElement.classList.add('light')
  }
})()`

const notoSansCJK = localFont({
  src: './fonts/NotoSansCJK-VF.otf',
  weight: '100 900',
  variable: '--font-noto-sans-cjk',
})

export const metadata: Metadata = {
  title: 'Norrix',
  icons: { icon: logo.src },
}

function RootLayout({
  children,
}: Readonly<{
  children: React.ReactNode
}>) {
  return (
    <html lang='en-US' suppressHydrationWarning style={{ backgroundColor: 'transparent' }}>
      <body
        className={`${notoSansCJK.variable} antialiased`}
        style={{ backgroundColor: 'transparent' }}
      >
        <Script id='theme-initialization' strategy='beforeInteractive'>
          {themeInitialization}
        </Script>
        <ThemeProvider>
          <Providers>{children}</Providers>
        </ThemeProvider>
      </body>
    </html>
  )
}

export default RootLayout
