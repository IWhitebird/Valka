import { RootProvider } from 'fumadocs-ui/provider/next';
import type { ReactNode } from 'react';
import type { Metadata } from 'next';
import { Instrument_Sans, JetBrains_Mono, Schibsted_Grotesk } from 'next/font/google';
import './global.css';

const instrumentSans = Instrument_Sans({ subsets: ['latin'], variable: '--font-instrument-sans' });
const schibstedGrotesk = Schibsted_Grotesk({
  subsets: ['latin'],
  variable: '--font-schibsted-grotesk',
});
const jetbrainsMono = JetBrains_Mono({ subsets: ['latin'], variable: '--font-jetbrains-mono' });

export const metadata: Metadata = {
  title: {
    template: '%s — Valka',
    default: 'Valka — Distributed Task Queue',
  },
  description:
    'A Rust-native distributed task queue whose only dependency is an S3 bucket. No database, no broker, nodes you can kill.',
};

export default function RootLayout({ children }: { children: ReactNode }) {
  return (
    <html
      lang="en"
      className={`${instrumentSans.variable} ${schibstedGrotesk.variable} ${jetbrainsMono.variable}`}
      suppressHydrationWarning
    >
      <body className="flex flex-col min-h-screen">
        <RootProvider>{children}</RootProvider>
      </body>
    </html>
  );
}
