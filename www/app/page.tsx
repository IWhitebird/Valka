import { HomeLayout } from 'fumadocs-ui/layouts/home';
import Link from 'next/link';
import { baseOptions } from '@/lib/layout.shared';
import {
  Archive,
  Zap,
  Code,
  Activity,
  ArrowRight,
  Github,
  Layers,
  Signal,
  RotateCcw,
  Globe,
  Shield,
  BarChart3,
} from 'lucide-react';
import { AnimateIn } from '@/components/animate-in';
import { SdkTabs } from '@/components/sdk-tabs';

const features = [
  {
    icon: Archive,
    title: 'One Bucket, No Database',
    description:
      'An S3-compatible bucket is the only source of truth. No Postgres, no Redis, no broker. Nodes are disposable and rebuild from the WAL.',
  },
  {
    icon: Zap,
    title: 'Zero-Latency Dispatch',
    description:
      'In-memory matching engine routes tasks to waiting workers instantly via oneshot channels.',
  },
  {
    icon: Globe,
    title: 'Polyglot SDKs',
    description:
      'First-class SDKs for Rust, TypeScript, Python, and Go. Same builder pattern, every language.',
  },
  {
    icon: Activity,
    title: 'Fully Observable',
    description:
      'Real-time log streaming, event bus, Prometheus metrics, and a built-in web dashboard.',
  },
  {
    icon: Signal,
    title: 'Task Signals',
    description:
      'Send real-time signals to running workers over the gRPC bidirectional stream.',
  },
  {
    icon: RotateCcw,
    title: 'Smart Retries & DLQ',
    description:
      'Configurable exponential backoff, dead letter queue, and automatic lease recovery.',
  },
];

const steps = [
  {
    step: '01',
    title: 'Ingest',
    description:
      'Tasks arrive via REST or gRPC, are appended to the write-ahead log, and are acknowledged once the segment is in the bucket. Matching starts immediately.',
  },
  {
    step: '02',
    title: 'Match',
    description:
      'Hot path: instant delivery to a waiting worker over an in-memory channel. Otherwise the task waits in the in-RAM pending index and is fed to workers as capacity frees.',
  },
  {
    step: '03',
    title: 'Execute',
    description:
      'Workers receive tasks over a single gRPC bidirectional stream. Heartbeats, logs, signals, and results all flow over one connection.',
  },
];

export default function Home() {
  return (
    <HomeLayout {...baseOptions()}>
      {/* ========================= HERO ========================= */}
      <section className="relative flex flex-col items-center px-6 pb-24 pt-28 text-center sm:pt-40">
        {/* Background layers */}
        <div className="pointer-events-none absolute inset-0 -z-10 overflow-hidden">
          {/* Dot grid */}
          <div className="hero-grid absolute inset-0" />
          {/* Gradient orbs */}
          <div className="float-orb absolute left-1/2 top-0 h-[600px] w-[900px] -translate-x-1/2 -translate-y-1/3 bg-[radial-gradient(ellipse,rgba(103,194,170,0.12),transparent_70%)]" />
          <div className="float-orb-reverse pulse-glow absolute left-1/4 top-1/4 size-[500px] -translate-x-1/2 bg-[radial-gradient(circle,rgba(62,159,136,0.08),transparent_70%)]" />
          <div className="float-orb pulse-glow absolute right-1/4 top-1/4 size-[500px] translate-x-1/2 bg-[radial-gradient(circle,rgba(194,122,72,0.07),transparent_70%)]" />
          {/* Bottom fade */}
          <div className="absolute bottom-0 left-0 right-0 h-32 bg-gradient-to-t from-[var(--color-fd-background)] to-transparent" />
        </div>

        {/* Badge */}
        <div className="hero-animate hero-animate-d1 mb-8 inline-flex items-center gap-2 rounded-full border border-white/10 bg-white/[0.03] px-4 py-1.5 text-sm text-fd-muted-foreground backdrop-blur-sm">
          <Layers className="size-3.5" />
          <span>Rust-native distributed task queue</span>
        </div>

        {/* Headline */}
        <h1 className="hero-animate hero-animate-d2 max-w-4xl text-5xl font-extrabold tracking-tight sm:text-6xl lg:text-7xl">
          The task queue that{' '}
          <span className="text-fd-primary">just works</span>
        </h1>

        {/* Subtitle */}
        <p className="hero-animate hero-animate-d3 mt-6 max-w-2xl text-lg leading-relaxed text-fd-muted-foreground sm:text-xl">
          An S3-compatible bucket is your only dependency. No database, no
          message broker, no cache layer. Just nodes you can kill at any time.
        </p>

        {/* CTA buttons */}
        <div className="hero-animate hero-animate-d4 mt-10 flex flex-wrap items-center justify-center gap-4">
          <Link
            href="/docs"
            className="group inline-flex items-center gap-2 rounded-lg bg-white px-6 py-3 text-sm font-semibold text-black transition-all hover:bg-white/90 hover:shadow-[0_0_24px_rgba(255,255,255,0.15)]"
          >
            Get Started
            <ArrowRight className="size-4 transition-transform group-hover:translate-x-0.5" />
          </Link>
          <a
            href="https://github.com/IWhitebird/Valka"
            target="_blank"
            rel="noopener noreferrer"
            className="inline-flex items-center gap-2 rounded-lg border border-white/10 bg-white/[0.03] px-6 py-3 text-sm font-semibold text-fd-foreground backdrop-blur-sm transition-all hover:border-white/20 hover:bg-white/[0.06]"
          >
            <Github className="size-4" />
            View on GitHub
          </a>
        </div>
      </section>

      {/* ========================= STATS BAR ========================= */}
      <div className="hero-animate hero-animate-d5 mx-auto max-w-5xl px-6">
        <div className="flex flex-wrap items-center justify-center gap-x-10 gap-y-4 rounded-xl border border-white/[0.06] bg-white/[0.02] px-8 py-5">
          {[
            { icon: Archive, label: 'One Bucket, Zero Databases' },
            { icon: Code, label: '4 SDK Languages' },
            { icon: Zap, label: 'gRPC Streaming' },
            { icon: Shield, label: 'Apache 2.0' },
            { icon: BarChart3, label: 'Built-in Dashboard' },
          ].map((s) => (
            <div
              key={s.label}
              className="flex items-center gap-2 text-sm text-fd-muted-foreground"
            >
              <s.icon className="size-4 text-fd-primary" />
              {s.label}
            </div>
          ))}
        </div>
      </div>

      {/* ========================= FEATURES ========================= */}
      <section className="mx-auto max-w-6xl px-6 py-28">
        <AnimateIn className="mb-14 text-center">
          <h2 className="text-3xl font-bold tracking-tight sm:text-4xl">
            Everything you need, nothing you don&apos;t
          </h2>
          <p className="mt-4 text-lg text-fd-muted-foreground">
            Built from the ground up for simplicity and performance.
          </p>
        </AnimateIn>

        <div className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
          {features.map((f, i) => (
            <AnimateIn key={f.title} delay={i * 0.08}>
              <div className="card-glow group relative h-full overflow-hidden rounded-xl border border-white/[0.06] bg-white/[0.02] p-6 transition-all duration-300 hover:border-white/[0.12] hover:bg-white/[0.04]">
                <div
                  className="pointer-events-none absolute inset-0 -z-10 bg-gradient-to-b from-fd-primary/15 to-fd-primary/0 opacity-0 transition-opacity duration-500 group-hover:opacity-100"
                />
                <div className="mb-4 inline-flex size-10 items-center justify-center rounded-lg bg-fd-primary/10">
                  <f.icon className="size-5 text-fd-primary" />
                </div>
                <h3 className="mb-2 font-semibold tracking-tight">{f.title}</h3>
                <p className="text-sm leading-relaxed text-fd-muted-foreground">
                  {f.description}
                </p>
              </div>
            </AnimateIn>
          ))}
        </div>
      </section>

      {/* ========================= SDK SHOWCASE ========================= */}
      <SdkTabs />

      {/* ========================= HOW IT WORKS ========================= */}
      <section className="mx-auto max-w-5xl px-6 py-28">
        <AnimateIn className="mb-14 text-center">
          <h2 className="text-3xl font-bold tracking-tight sm:text-4xl">How it works</h2>
          <p className="mt-4 text-lg text-fd-muted-foreground">
            Two paths, one goal: get tasks to workers as fast as possible.
          </p>
        </AnimateIn>

        <div className="grid gap-6 md:grid-cols-3">
          {steps.map((s, i) => (
            <AnimateIn key={s.step} delay={i * 0.12}>
              <div
                className="card-glow group relative h-full overflow-hidden rounded-xl border border-fd-primary/20 bg-white/[0.02] p-6 transition-all duration-300 hover:bg-white/[0.04]"
              >
                <div className="mb-5 inline-flex size-12 items-center justify-center rounded-xl bg-fd-primary/10">
                  <span className="text-xl font-black text-fd-primary">{s.step}</span>
                </div>
                <h3 className="mb-3 text-lg font-semibold tracking-tight">{s.title}</h3>
                <p className="text-sm leading-relaxed text-fd-muted-foreground">
                  {s.description}
                </p>
              </div>
            </AnimateIn>
          ))}
        </div>

        <AnimateIn className="mt-8 text-center" delay={0.3}>
          <Link
            href="/docs/architecture"
            className="group inline-flex items-center gap-1.5 text-sm font-medium text-fd-muted-foreground transition-colors hover:text-fd-foreground"
          >
            Read the full architecture docs
            <ArrowRight className="size-3.5 transition-transform group-hover:translate-x-0.5" />
          </Link>
        </AnimateIn>
      </section>

      {/* ========================= CTA ========================= */}
      <section className="relative flex flex-col items-center px-6 pb-32 pt-16 text-center">
        <div className="pointer-events-none absolute inset-0 -z-10 overflow-hidden">
          <div className="float-orb-reverse absolute bottom-0 left-1/2 h-[400px] w-[700px] -translate-x-1/2 translate-y-1/3 bg-[radial-gradient(ellipse,rgba(103,194,170,0.1),transparent_70%)]" />
        </div>

        <AnimateIn>
          <h2 className="text-3xl font-bold tracking-tight sm:text-4xl">
            Ready to simplify your task queue?
          </h2>
          <p className="mx-auto mt-4 mb-10 max-w-lg text-lg text-fd-muted-foreground">
            Read the docs, spin up a server, and ship your first worker in minutes.
          </p>
          <div className="flex flex-wrap items-center justify-center gap-4">
            <Link
              href="/docs"
              className="group inline-flex items-center gap-2 rounded-lg bg-white px-6 py-3 text-sm font-semibold text-black transition-all hover:bg-white/90 hover:shadow-[0_0_24px_rgba(255,255,255,0.15)]"
            >
              Read the Docs
              <ArrowRight className="size-4 transition-transform group-hover:translate-x-0.5" />
            </Link>
            <Link
              href="/docs/quick-start"
              className="inline-flex items-center gap-2 rounded-lg border border-white/10 bg-white/[0.03] px-6 py-3 text-sm font-semibold text-fd-foreground backdrop-blur-sm transition-all hover:border-white/20 hover:bg-white/[0.06]"
            >
              Quick Start Guide
            </Link>
          </div>
        </AnimateIn>
      </section>
    </HomeLayout>
  );
}
