import type { NextConfig } from 'next'

const nextConfig: NextConfig = {
  devIndicators: false,
  transpilePackages: ['@norrix/bridge', '@norrix/ui'],
  output: 'export',
  images: {
    unoptimized: true,
  },
}

export default nextConfig
