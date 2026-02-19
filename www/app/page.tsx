import { HomeLayout } from 'fumadocs-ui/layouts/home';
import Link from 'next/link';
import { baseOptions } from '@/lib/layout.shared';
import {
  Database,
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
    icon: Database,
    title: 'PostgreSQL Only',
    description:
      'Single source of truth. No Redis, no RabbitMQ, no NATS. One dependency to deploy and manage.',
    gradient: 'from-emerald-500/20 to-emerald-500/0',
    iconColor: 'text-emerald-400',
    iconBg: 'bg-emerald-500/10',
  },
  {
    icon: Zap,
    title: 'Zero-Latency Dispatch',
    description:
      'In-memory matching engine routes tasks to waiting workers instantly via oneshot channels.',
    gradient: 'from-amber-500/20 to-amber-500/0',
    iconColor: 'text-amber-400',
    iconBg: 'bg-amber-500/10',
  },
  {
    icon: Globe,
    title: 'Polyglot SDKs',
    description:
      'First-class SDKs for Rust, TypeScript, Python, and Go. Same builder pattern, every language.',
    gradient: 'from-violet-500/20 to-violet-500/0',
    iconColor: 'text-violet-400',
    iconBg: 'bg-violet-500/10',
  },
  {
    icon: Activity,
    title: 'Fully Observable',
    description:
      'Real-time log streaming, event bus, Prometheus metrics, and a built-in web dashboard.',
    gradient: 'from-sky-500/20 to-sky-500/0',
    iconColor: 'text-sky-400',
    iconBg: 'bg-sky-500/10',
  },
  {
    icon: Signal,
    title: 'Task Signals',
    description:
      'Send real-time signals to running workers over the gRPC bidirectional stream.',
    gradient: 'from-rose-500/20 to-rose-500/0',
    iconColor: 'text-rose-400',
    iconBg: 'bg-rose-500/10',
  },
  {
    icon: RotateCcw,
    title: 'Smart Retries & DLQ',
    description:
      'Configurable exponential backoff, dead letter queue, and automatic lease recovery.',
    gradient: 'from-orange-500/20 to-orange-500/0',
    iconColor: 'text-orange-400',
    iconBg: 'bg-orange-500/10',
  },
];

const steps = [
  {
    step: '01',
    title: 'Ingest',
    description:
      'Tasks arrive via REST or gRPC and are durably persisted to PostgreSQL. The in-memory MatchingService checks for waiting workers immediately.',
    accent: 'text-sky-400',
    iconBg: 'bg-sky-500/10',
    borderColor: 'border-sky-500/20',
  },
  {
    step: '02',
    title: 'Match',
    description:
      'Hot path: instant delivery via in-memory oneshot channel. Cold path: workers poll with PG SKIP LOCKED for guaranteed delivery.',
    accent: 'text-violet-400',
    iconBg: 'bg-violet-500/10',
    borderColor: 'border-violet-500/20',
  },
  {
    step: '03',
    title: 'Execute',
    description:
      'Workers receive tasks over a single gRPC bidirectional stream. Heartbeats, logs, signals, and results all flow over one connection.',
    accent: 'text-emerald-400',
    iconBg: 'bg-emerald-500/10',
    borderColor: 'border-emerald-500/20',
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
          <div className="float-orb absolute left-1/2 top-0 h-[600px] w-[900px] -translate-x-1/2 -translate-y-1/3 bg-[radial-gradient(ellipse,rgba(99,102,241,0.12),transparent_70%)]" />
          <div className="float-orb-reverse pulse-glow absolute left-1/4 top-1/4 size-[500px] -translate-x-1/2 bg-[radial-gradient(circle,rgba(56,189,248,0.08),transparent_70%)]" />
          <div className="float-orb pulse-glow absolute right-1/4 top-1/4 size-[500px] translate-x-1/2 bg-[radial-gradient(circle,rgba(192,132,252,0.08),transparent_70%)]" />
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
          <span className="gradient-text-animated bg-gradient-to-r from-[#38bdf8] via-[#818cf8] to-[#c084fc] bg-clip-text text-transparent">
            just works
          </span>
        </h1>

        {/* Subtitle */}
        <p className="hero-animate hero-animate-d3 mt-6 max-w-2xl text-lg leading-relaxed text-fd-muted-foreground sm:text-xl">
          PostgreSQL is your only dependency. No message broker, no cache layer,
          no complexity. Just a task queue that scales.
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
            { icon: Database, label: 'PostgreSQL Only', color: 'text-emerald-400' },
            { icon: Code, label: '4 SDK Languages', color: 'text-violet-400' },
            { icon: Zap, label: 'gRPC Streaming', color: 'text-amber-400' },
            { icon: Shield, label: 'Apache 2.0', color: 'text-sky-400' },
            { icon: BarChart3, label: 'Built-in Dashboard', color: 'text-rose-400' },
          ].map((s) => (
            <div
              key={s.label}
              className="flex items-center gap-2 text-sm text-fd-muted-foreground"
            >
              <s.icon className={`size-4 ${s.color}`} />
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
                  className={`pointer-events-none absolute inset-0 -z-10 bg-gradient-to-b ${f.gradient} opacity-0 transition-opacity duration-500 group-hover:opacity-100`}
                />
                <div
                  className={`mb-4 inline-flex size-10 items-center justify-center rounded-lg ${f.iconBg}`}
                >
                  <f.icon className={`size-5 ${f.iconColor}`} />
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
                className={`card-glow group relative h-full overflow-hidden rounded-xl border ${s.borderColor} bg-white/[0.02] p-6 transition-all duration-300 hover:bg-white/[0.04]`}
              >
                <div
                  className={`mb-5 inline-flex size-12 items-center justify-center rounded-xl ${s.iconBg}`}
                >
                  <span className={`text-xl font-black ${s.accent}`}>{s.step}</span>
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
          <div className="float-orb-reverse absolute bottom-0 left-1/2 h-[400px] w-[700px] -translate-x-1/2 translate-y-1/3 bg-[radial-gradient(ellipse,rgba(99,102,241,0.1),transparent_70%)]" />
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
