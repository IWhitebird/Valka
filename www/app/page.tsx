import { HomeLayout } from 'fumadocs-ui/layouts/home';
import Link from 'next/link';
import { baseOptions } from '@/lib/layout.shared';
import { ArrowRight, Github } from 'lucide-react';
import { AnimateIn } from '@/components/animate-in';
import { SdkTabs } from '@/components/sdk-tabs';

// Real output: `valka cluster status` a few seconds after `kill -9` of one node of the
// compose cluster (columns trimmed to fit).
const failover = [
  { prompt: true, text: 'export COMPOSE_FILE=docker-compose.cluster.yml' },
  { prompt: true, text: 'docker compose up -d' },
  { prompt: true, text: 'export VALKA_HTTP=http://localhost:8981' },
  { prompt: true, text: 'export VALKA_API_KEY=change-me-api-key' },
  { prompt: true, text: 'docker compose kill -s SIGKILL valka-2' },
  { prompt: true, text: 'valka cluster status' },
  { prompt: false, text: 'Cluster 01a1605b…  health OK  shards 4096  clustered' },
  { prompt: false, text: '' },
  { prompt: false, text: 'NODE       INC STATUS   SHARDS  PENDING  RUNNING' },
  { prompt: false, text: 'valka-1      7 alive      1409        1        0' },
  { prompt: false, text: 'valka-3      8 alive      2687        2        0' },
  { prompt: false, text: 'valka-2      7 dead          -        -        -' },
];

const guarantees = [
  {
    title: 'Saved before it is acknowledged',
    body: 'Every create, result and cancel is a record in a write-ahead log stored in your bucket. A node answers OK only after the segment holding it is written and the node has re-read its own lease, so a node that was declared dead cannot acknowledge anything.',
  },
  {
    title: 'One writer per shard',
    body: 'State is split into shards. Each has exactly one owner, recorded in the bucket and moved only by compare-and-swap. No database, no Raft, no gossip, no leader: nodes that share nothing but a bucket still never write the same shard.',
  },
  {
    title: 'Work survives the failure',
    body: 'Workers keep running when their node dies, reconnect anywhere and reattach their tasks. Results are acknowledged end to end, and every create carries an idempotency key, so retries never make a second task.',
  },
];

const bucket = `cluster.json                      cluster id, shard count
nodes/valka-2/lease               liveness: incarnation, renewals
owners/b1/02731                   owner and generation of shard 2731
wal/valka-2/…07-…0120.seg         one group commit of records
snapshots/b1/02731/…-split.snap   shard 2731, rebuilt from a dead log
splits/valka-2/7                  incarnation 7 is dead and split
queues/3f/emails                  partition count of a queue
logs/…/{run}/…                    task log lines`;

const writePath = [
  { label: 'Your request', detail: 'REST or gRPC, to any node' },
  { label: 'Shard owner', detail: 'validates and applies it in memory' },
  { label: 'WAL segment', detail: 'group commit to the bucket, ≤ 50 ms' },
  { label: 'Lease check', detail: 'still the owner? only then OK' },
  { label: 'A worker', detail: 'gets the task the moment it is saved' },
];

const capabilities = [
  ['Steps', 'Checkpoint each step; a retry skips the finished ones.'],
  ['Signals', 'Send messages to a running task and get acknowledgements.'],
  ['Retries and dead letters', 'Exponential backoff, then a dead-letter queue.'],
  ['Scheduled tasks', 'Run no earlier than a given time.'],
  ['Idempotency keys', 'One task per key, exact across the whole cluster.'],
  ['Queue partitions', 'Spread a hot queue over nodes; resize online.'],
  ['Dashboard and CLI', 'Tasks, workers, shard map, drains and shard moves.'],
  ['Metrics and runbooks', 'Prometheus metrics, alert rules, what to do when they fire.'],
  ['API keys', 'Keys for producers and workers, a separate admin token.'],
];

const checks = [
  {
    value: '7',
    label: 'protocol bugs found by the TLA+ model',
    detail: 'before any cluster code existed; the model runs in CI',
  },
  {
    value: '4',
    label: 'found by a randomized simulation',
    detail: 'thousands of seeds of kills, freezes and bucket faults, with a history checker',
  },
  {
    value: '3',
    label: 'found by chaos on real processes',
    detail: 'kill -9, freezes and restarts under load on a three-node cluster',
  },
  {
    value: '7',
    label: 'found by load and failover benchmarks',
    detail: 'a livelock, 30-second stalls, a per-worker ceiling, slow takeovers',
  },
];

function SectionHeading({ title, lead }: { title: string; lead?: string }) {
  return (
    <AnimateIn className="mb-10 max-w-2xl">
      <h2 className="text-balance text-3xl font-extrabold tracking-tight">{title}</h2>
      {lead && <p className="mt-3 text-lg text-fd-muted-foreground">{lead}</p>}
    </AnimateIn>
  );
}

export default function Home() {
  return (
    <HomeLayout {...baseOptions()}>
      {/* Hero: the claim, and what it looks like when a node dies. */}
      <section className="mx-auto grid max-w-6xl items-center gap-12 px-6 pb-20 pt-16 sm:pt-24 lg:grid-cols-[1.05fr_1fr]">
        <div>
          <p className="mb-5 font-mono text-sm text-fd-primary">Open source · Apache-2.0 · written in Rust</p>
          <h1 className="text-balance text-4xl font-extrabold tracking-tight sm:text-5xl lg:text-[3.4rem] lg:leading-[1.05]">
            A task queue whose only dependency is a bucket
          </h1>
          <p className="mt-6 max-w-xl text-lg leading-relaxed text-fd-muted-foreground">
            Every task, retry and result is in your S3-compatible bucket before Valka says OK.
            Nodes hold their shards in memory and are disposable: kill one, and the others take
            its work over once its lease runs out.
          </p>
          <div className="mt-9 flex flex-wrap items-center gap-3">
            <Link
              href="/docs/quick-start"
              className="group inline-flex items-center gap-2 rounded-lg bg-fd-primary px-5 py-2.5 text-sm font-semibold text-fd-primary-foreground transition-opacity hover:opacity-90"
            >
              Get started
              <ArrowRight className="size-4 transition-transform group-hover:translate-x-0.5" />
            </Link>
            <Link
              href="/docs/architecture"
              className="inline-flex items-center gap-2 rounded-lg border border-fd-border px-5 py-2.5 text-sm font-semibold transition-colors hover:bg-fd-accent"
            >
              How it works
            </Link>
            <a
              href="https://github.com/IWhitebird/Valka"
              target="_blank"
              rel="noopener noreferrer"
              className="inline-flex items-center gap-2 rounded-lg px-3 py-2.5 text-sm font-semibold text-fd-muted-foreground transition-colors hover:text-fd-foreground"
            >
              <Github className="size-4" />
              GitHub
            </a>
          </div>
        </div>

        <div className="min-w-0 overflow-hidden rounded-xl border border-fd-border bg-[#0d1211] shadow-sm">
          <div className="flex items-center gap-1.5 border-b border-white/[0.06] px-4 py-2.5">
            <span className="size-2.5 rounded-full bg-white/15" />
            <span className="size-2.5 rounded-full bg-white/15" />
            <span className="size-2.5 rounded-full bg-white/15" />
            <span className="ml-3 font-mono text-xs text-[#94a19c]">three nodes, one bucket</span>
          </div>
          <pre className="overflow-x-auto p-5 font-mono text-[12.5px] leading-relaxed text-[#d7dfdb]">
            {failover.map((l, i) => (
              <div key={i}>
                {l.prompt ? (
                  <>
                    <span className="text-[#67c2aa]">$ </span>
                    {l.text}
                  </>
                ) : (
                  <span className={l.text.includes('dead') ? 'text-[#e2a47a]' : 'text-[#94a19c]'}>
                    {l.text || ' '}
                  </span>
                )}
              </div>
            ))}
          </pre>
        </div>
      </section>

      {/* Guarantees: the mechanisms, not adjectives. */}
      <section className="border-y border-fd-border bg-fd-card/40">
        <div className="mx-auto grid max-w-6xl gap-10 px-6 py-16 md:grid-cols-3">
          {guarantees.map((g, i) => (
            <AnimateIn key={g.title} delay={i * 0.06}>
              <h3 className="text-lg font-bold tracking-tight">{g.title}</h3>
              <p className="mt-3 text-[15px] leading-relaxed text-fd-muted-foreground">{g.body}</p>
            </AnimateIn>
          ))}
        </div>
      </section>

      {/* The write path. */}
      <section className="mx-auto max-w-6xl px-6 py-20">
        <SectionHeading
          title="What happens to a write"
          lead="The same path for a create, a result, a cancel or a retry timer firing."
        />
        <ol className="grid gap-3 md:grid-cols-5">
          {writePath.map((s, i) => (
            <AnimateIn key={s.label} delay={i * 0.05}>
              <li className="relative h-full rounded-lg border border-fd-border bg-fd-card px-4 py-4">
                <p className="font-semibold">{s.label}</p>
                <p className="mt-1 text-sm text-fd-muted-foreground">{s.detail}</p>
                {i < writePath.length - 1 && (
                  <ArrowRight
                    aria-hidden
                    className="absolute -right-3 top-1/2 z-10 hidden size-4 -translate-y-1/2 text-fd-muted-foreground md:block"
                  />
                )}
              </li>
            </AnimateIn>
          ))}
        </ol>
      </section>

      {/* The bucket. */}
      <section className="mx-auto grid max-w-6xl gap-10 px-6 pb-20 lg:grid-cols-[1fr_1.4fr]">
        <AnimateIn>
          <h2 className="text-balance text-3xl font-extrabold tracking-tight">
            Everything lives in your bucket
          </h2>
          <p className="mt-4 text-lg leading-relaxed text-fd-muted-foreground">
            Plain objects you can list, read and back up. The log is compressed JSON, so any tool
            can follow it. Run one node against a local directory while you build, and a cluster
            against S3, MinIO or another compatible store in production.
          </p>
          <Link
            href="/docs/clustering"
            className="group mt-6 inline-flex items-center gap-1.5 text-sm font-semibold text-fd-primary"
          >
            How nodes share it
            <ArrowRight className="size-3.5 transition-transform group-hover:translate-x-0.5" />
          </Link>
        </AnimateIn>
        <AnimateIn delay={0.08} className="min-w-0">
          <pre className="overflow-x-auto rounded-xl border border-fd-border bg-fd-card p-5 font-mono text-[12.5px] leading-relaxed text-fd-muted-foreground">
            {bucket}
          </pre>
        </AnimateIn>
      </section>

      {/* SDKs. */}
      <SdkTabs />

      {/* Capabilities. */}
      <section className="mx-auto max-w-6xl px-6 py-20">
        <SectionHeading title="Built for real work" />
        <dl className="grid gap-x-10 gap-y-6 sm:grid-cols-2 lg:grid-cols-3">
          {capabilities.map(([term, desc]) => (
            <div key={term} className="border-t border-fd-border pt-4">
              <dt className="font-semibold">{term}</dt>
              <dd className="mt-1 text-sm text-fd-muted-foreground">{desc}</dd>
            </div>
          ))}
        </dl>
      </section>

      {/* Verification. */}
      <section className="border-y border-fd-border bg-fd-card/40">
        <div className="mx-auto max-w-6xl px-6 py-20">
          <SectionHeading
            title="Tested the way it fails"
            lead="Each bug below was found before release and fixed with a regression test."
          />
          <div className="grid gap-8 sm:grid-cols-2 lg:grid-cols-4">
            {checks.map((c) => (
              <AnimateIn key={c.label}>
                <p className="font-display text-5xl font-extrabold tabular-nums text-fd-primary">{c.value}</p>
                <p className="mt-2 font-semibold">{c.label}</p>
                <p className="mt-1 text-sm text-fd-muted-foreground">{c.detail}</p>
              </AnimateIn>
            ))}
          </div>
        </div>
      </section>

      {/* Quick start. */}
      <section className="mx-auto max-w-6xl px-6 py-20">
        <div className="grid gap-10 lg:grid-cols-2">
          <AnimateIn>
            <h2 className="text-balance text-3xl font-extrabold tracking-tight">Try it in a minute</h2>
            <p className="mt-4 text-lg text-fd-muted-foreground">
              One node against a local directory, or three nodes and an S3-compatible store in
              Docker. The dashboard runs on every node.
            </p>
            <div className="mt-8 flex flex-wrap gap-3">
              <Link
                href="/docs/quick-start"
                className="group inline-flex items-center gap-2 rounded-lg bg-fd-primary px-5 py-2.5 text-sm font-semibold text-fd-primary-foreground transition-opacity hover:opacity-90"
              >
                Quick start
                <ArrowRight className="size-4 transition-transform group-hover:translate-x-0.5" />
              </Link>
              <Link
                href="/docs/deployment"
                className="inline-flex items-center gap-2 rounded-lg border border-fd-border px-5 py-2.5 text-sm font-semibold transition-colors hover:bg-fd-accent"
              >
                Deployment
              </Link>
            </div>
          </AnimateIn>
          <AnimateIn delay={0.08} className="min-w-0">
            <pre className="overflow-x-auto rounded-xl border border-fd-border bg-fd-card p-5 font-mono text-[13px] leading-relaxed">
              <span className="text-fd-muted-foreground"># one node, data in ./data</span>
              {'\n'}cargo run -p valka-server{'\n\n'}
              <span className="text-fd-muted-foreground"># three nodes: dashboard on :8981, :8982, :8983</span>
              {'\n'}docker compose -f docker-compose.cluster.yml up -d{'\n\n'}
              <span className="text-fd-muted-foreground"># break it and watch it recover</span>
              {'\n'}scripts/chaos.sh
            </pre>
          </AnimateIn>
        </div>
      </section>
    </HomeLayout>
  );
}
