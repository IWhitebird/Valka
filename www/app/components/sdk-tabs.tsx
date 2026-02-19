'use client';

import { useState } from 'react';
import { AnimateIn } from './animate-in';

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

export function SdkTabs() {
  const [activeTab, setActiveTab] = useState<Lang>('Rust');

  return (
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
  );
}
