import type { Route } from './+types/home';
import { HomeLayout } from 'fumadocs-ui/layouts/home';
import { Link } from 'react-router';
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
import { useState, useEffect, useRef, type ReactNode } from 'react';

export function meta({}: Route.MetaArgs) {
  return [
    { title: 'Valka — Distributed Task Queue' },
    {
      name: 'description',
      content:
        'A Rust-native distributed task queue powered by PostgreSQL. One dependency. Zero brokers. Built for simplicity.',
    },
  ];
}

// ---------------------------------------------------------------------------
// AnimateIn — scroll-triggered fade-in component
// ---------------------------------------------------------------------------
function AnimateIn({
  children,
  className = '',
  delay = 0,
}: {
  children: ReactNode;
  className?: string;
  delay?: number;
}) {
  const ref = useRef<HTMLDivElement>(null);
  const [visible, setVisible] = useState(false);

  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const observer = new IntersectionObserver(
      ([entry]) => {
        if (entry.isIntersecting) {
          setVisible(true);
          observer.disconnect();
        }
      },
      { threshold: 0.1, rootMargin: '0px 0px -60px 0px' },
    );
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  return (
    <div
      ref={ref}
      className={className}
      style={{
        opacity: visible ? 1 : 0,
        transform: visible ? 'none' : 'translateY(28px)',
        transition: `opacity 0.7s cubic-bezier(0.16, 1, 0.3, 1) ${delay}s, transform 0.7s cubic-bezier(0.16, 1, 0.3, 1) ${delay}s`,
      }}
    >
      {children}
    </div>
  );
}

// ---------------------------------------------------------------------------
// Feature data
// ---------------------------------------------------------------------------
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

// ---------------------------------------------------------------------------
// Language tabs
// ---------------------------------------------------------------------------
const languages = ['Rust', 'TypeScript', 'Python', 'Go'] as const;
type Lang = (typeof languages)[number];

const langMeta: Record<Lang, { color: string; activeColor: string; label: string }> = {
  Rust: {
    color: 'text-fd-muted-foreground',
    activeColor: 'text-orange-400 border-orange-400',
    label: 'RS',
  },
  TypeScript: {
    color: 'text-fd-muted-foreground',
    activeColor: 'text-blue-400 border-blue-400',
    label: 'TS',
  },
  Python: {
    color: 'text-fd-muted-foreground',
    activeColor: 'text-yellow-400 border-yellow-400',
    label: 'PY',
  },
  Go: {
    color: 'text-fd-muted-foreground',
    activeColor: 'text-cyan-400 border-cyan-400',
    label: 'GO',
  },
};

// ---------------------------------------------------------------------------
// Syntax-highlighted code snippets (static HTML)
// ---------------------------------------------------------------------------
const codeSnippets: Record<Lang, string> = {
  Rust: `<span class="syn-kw">use</span> valka_sdk::{<span class="syn-typ">TaskContext</span>, <span class="syn-typ">ValkaWorker</span>};

<span class="syn-kw">let</span> worker = <span class="syn-typ">ValkaWorker</span>::builder()
    .name(<span class="syn-str">"email-worker"</span>)
    .server_addr(<span class="syn-str">"http://localhost:50051"</span>)
    .queues(&amp;[<span class="syn-str">"emails"</span>])
    .concurrency(<span class="syn-num">8</span>)
    .handler(|ctx: <span class="syn-typ">TaskContext</span>| <span class="syn-kw">async</span> <span class="syn-kw">move</span> {
        <span class="syn-kw">let</span> input: serde_json::<span class="syn-typ">Value</span> = ctx.input()?;
        <span class="syn-cmt">// process task ...</span>
        <span class="syn-typ">Ok</span>(serde_json::json!({<span class="syn-str">"status"</span>: <span class="syn-str">"sent"</span>}))
    })
    .build()
    .<span class="syn-kw">await</span>?;

worker.run().<span class="syn-kw">await</span>?;`,

  TypeScript: `<span class="syn-kw">import</span> { <span class="syn-typ">ValkaWorker</span>, <span class="syn-kw">type</span> <span class="syn-typ">TaskContext</span> } <span class="syn-kw">from</span> <span class="syn-str">"@valka/sdk"</span>;

<span class="syn-kw">const</span> worker = <span class="syn-typ">ValkaWorker</span>.builder()
  .name(<span class="syn-str">"email-worker"</span>)
  .serverAddr(<span class="syn-str">"localhost:50051"</span>)
  .queues([<span class="syn-str">"emails"</span>])
  .concurrency(<span class="syn-num">8</span>)
  .handler(<span class="syn-kw">async</span> (ctx: <span class="syn-typ">TaskContext</span>) =&gt; {
    <span class="syn-kw">const</span> input = ctx.input&lt;{ to: <span class="syn-typ">string</span> }&gt;();
    <span class="syn-cmt">// process task ...</span>
    <span class="syn-kw">return</span> { status: <span class="syn-str">"sent"</span> };
  })
  .build();

<span class="syn-kw">await</span> worker.run();`,

  Python: `<span class="syn-kw">from</span> valka <span class="syn-kw">import</span> <span class="syn-typ">ValkaWorker</span>, <span class="syn-typ">TaskContext</span>

<span class="syn-kw">async def</span> <span class="syn-fn">handle_task</span>(ctx: <span class="syn-typ">TaskContext</span>) -&gt; <span class="syn-typ">dict</span>:
    data = ctx.input()
    <span class="syn-cmt"># process task ...</span>
    <span class="syn-kw">return</span> {<span class="syn-str">"status"</span>: <span class="syn-str">"sent"</span>}

worker = (
    <span class="syn-typ">ValkaWorker</span>.builder()
    .name(<span class="syn-str">"email-worker"</span>)
    .server_addr(<span class="syn-str">"localhost:50051"</span>)
    .queues([<span class="syn-str">"emails"</span>])
    .concurrency(<span class="syn-num">8</span>)
    .handler(handle_task)
    .build()
)
<span class="syn-kw">await</span> worker.run()`,

  Go: `worker, _ := valka.<span class="syn-fn">NewWorker</span>(
    valka.<span class="syn-fn">WithName</span>(<span class="syn-str">"email-worker"</span>),
    valka.<span class="syn-fn">WithServerAddr</span>(<span class="syn-str">"localhost:50051"</span>),
    valka.<span class="syn-fn">WithQueues</span>(<span class="syn-str">"emails"</span>),
    valka.<span class="syn-fn">WithConcurrency</span>(<span class="syn-num">8</span>),
    valka.<span class="syn-fn">WithHandler</span>(<span class="syn-kw">func</span>(ctx *valka.<span class="syn-typ">TaskContext</span>) (<span class="syn-typ">any</span>, <span class="syn-typ">error</span>) {
        <span class="syn-kw">var</span> input <span class="syn-typ">map</span>[<span class="syn-typ">string</span>]<span class="syn-typ">any</span>
        ctx.Input(&amp;input)
        <span class="syn-cmt">// process task ...</span>
        <span class="syn-kw">return</span> <span class="syn-typ">map</span>[<span class="syn-typ">string</span>]<span class="syn-typ">any</span>{<span class="syn-str">"status"</span>: <span class="syn-str">"sent"</span>}, <span class="syn-kw">nil</span>
    }),
)
worker.Run(context.Background())`,
};

// ---------------------------------------------------------------------------
// Architecture steps
// ---------------------------------------------------------------------------
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

// ---------------------------------------------------------------------------
// Component
// ---------------------------------------------------------------------------
export default function Home() {
  const [activeTab, setActiveTab] = useState<Lang>('Rust');

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
            to="/docs"
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
      <section className="mx-auto max-w-5xl px-6 py-20">
        <AnimateIn className="mb-12 text-center">
          <h2 className="text-3xl font-bold tracking-tight sm:text-4xl">
            One API, every language
          </h2>
          <p className="mt-4 text-lg text-fd-muted-foreground">
            Same builder pattern across Rust, TypeScript, Python, and Go.
          </p>
        </AnimateIn>

        <AnimateIn delay={0.1}>
          <div className="overflow-hidden rounded-xl border border-white/[0.06] bg-[#0a0a0a]">
            {/* Language tabs */}
            <div className="flex gap-1 border-b border-white/[0.06] bg-white/[0.02] px-3 pt-3">
              {languages.map((lang) => {
                const active = activeTab === lang;
                return (
                  <button
                    key={lang}
                    onClick={() => setActiveTab(lang)}
                    className={`relative flex items-center gap-2 rounded-t-lg px-4 py-2.5 text-sm font-medium transition-all ${
                      active
                        ? 'bg-[#0a0a0a] text-fd-foreground'
                        : 'text-fd-muted-foreground hover:text-fd-foreground'
                    }`}
                  >
                    <span
                      className={`inline-flex size-5 items-center justify-center rounded text-[10px] font-bold transition-colors ${
                        active ? langMeta[lang].activeColor : langMeta[lang].color
                      }`}
                    >
                      {langMeta[lang].label}
                    </span>
                    {lang}
                    {active && (
                      <span className="absolute bottom-0 left-3 right-3 h-px bg-fd-primary" />
                    )}
                  </button>
                );
              })}
            </div>

            {/* Code block */}
            <div className="overflow-x-auto p-6">
              <pre className="text-[13px] leading-relaxed">
                <code dangerouslySetInnerHTML={{ __html: codeSnippets[activeTab] }} />
              </pre>
            </div>
          </div>
        </AnimateIn>
      </section>

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
            to="/docs/architecture"
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
              to="/docs"
              className="group inline-flex items-center gap-2 rounded-lg bg-white px-6 py-3 text-sm font-semibold text-black transition-all hover:bg-white/90 hover:shadow-[0_0_24px_rgba(255,255,255,0.15)]"
            >
              Read the Docs
              <ArrowRight className="size-4 transition-transform group-hover:translate-x-0.5" />
            </Link>
            <Link
              to="/docs/quick-start"
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
