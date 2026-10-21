'use client';

import { useState } from 'react';
import { AnimateIn } from './animate-in';

const languages = ['Rust', 'TypeScript', 'Python', 'Go'] as const;
type Lang = (typeof languages)[number];

const install: Record<Lang, string> = {
  Rust: 'cargo add valka-sdk --git https://github.com/IWhitebird/Valka',
  TypeScript: 'npm install https://github.com/IWhitebird/Valka/releases/download/v0.2.0/valka-sdk-0.2.0.tgz',
  Python: 'pip install "git+https://github.com/IWhitebird/Valka#subdirectory=sdks/python"',
  Go: 'go get github.com/IWhitebird/Valka/sdks/go',
};

// Hand-highlighted: each snippet compiles against the SDK in this repository.
const snippets: Record<Lang, string> = {
  Rust: `<span class="syn-kw">let</span> worker = <span class="syn-typ">ValkaWorker</span>::builder()
    .name(<span class="syn-str">"email-worker"</span>)
    .server_addrs(&amp;[<span class="syn-str">"http://valka-1:50051"</span>, <span class="syn-str">"http://valka-2:50051"</span>])
    .queues(&amp;[<span class="syn-str">"emails"</span>])
    .concurrency(<span class="syn-num">8</span>)
    .handler(|ctx| <span class="syn-kw">async move</span> {
        <span class="syn-kw">let</span> input: <span class="syn-typ">Email</span> = ctx.input().map_err(|e| e.to_string())?;
        send(&amp;input).<span class="syn-kw">await</span>;
        <span class="syn-typ">Ok</span>(json!({ <span class="syn-str">"sent"</span>: <span class="syn-kw">true</span> }))
    })
    .build()
    .<span class="syn-kw">await</span>?;
worker.run().<span class="syn-kw">await</span>?;`,

  TypeScript: `<span class="syn-kw">import</span> { <span class="syn-typ">ValkaWorker</span> } <span class="syn-kw">from</span> <span class="syn-str">"@valka/sdk"</span>;

<span class="syn-kw">const</span> worker = <span class="syn-typ">ValkaWorker</span>.builder()
  .name(<span class="syn-str">"email-worker"</span>)
  .serverAddrs([<span class="syn-str">"valka-1:50051"</span>, <span class="syn-str">"valka-2:50051"</span>])
  .queues([<span class="syn-str">"emails"</span>])
  .concurrency(<span class="syn-num">8</span>)
  .handler(<span class="syn-kw">async</span> (ctx) =&gt; {
    <span class="syn-kw">const</span> input = ctx.input&lt;<span class="syn-typ">Email</span>&gt;();
    <span class="syn-kw">await</span> send(input);
    <span class="syn-kw">return</span> { sent: <span class="syn-kw">true</span> };
  })
  .build();
<span class="syn-kw">await</span> worker.run();`,

  Python: `<span class="syn-kw">import</span> asyncio
<span class="syn-kw">from</span> valka <span class="syn-kw">import</span> <span class="syn-typ">TaskContext</span>, <span class="syn-typ">ValkaWorker</span>

<span class="syn-kw">async def</span> <span class="syn-fn">handle</span>(ctx: <span class="syn-typ">TaskContext</span>) -&gt; <span class="syn-typ">dict</span>:
    <span class="syn-kw">await</span> send(ctx.input())
    <span class="syn-kw">return</span> {<span class="syn-str">"sent"</span>: <span class="syn-kw">True</span>}

worker = (
    <span class="syn-typ">ValkaWorker</span>.builder()
    .name(<span class="syn-str">"email-worker"</span>)
    .server_addrs([<span class="syn-str">"valka-1:50051"</span>, <span class="syn-str">"valka-2:50051"</span>])
    .queues([<span class="syn-str">"emails"</span>])
    .concurrency(<span class="syn-num">8</span>)
    .handler(handle)
    .build()
)
asyncio.run(worker.run())`,

  Go: `worker, err := valka.<span class="syn-fn">NewWorker</span>(
    valka.<span class="syn-fn">WithName</span>(<span class="syn-str">"email-worker"</span>),
    valka.<span class="syn-fn">WithServerAddrs</span>(<span class="syn-str">"valka-1:50051"</span>, <span class="syn-str">"valka-2:50051"</span>),
    valka.<span class="syn-fn">WithQueues</span>(<span class="syn-str">"emails"</span>),
    valka.<span class="syn-fn">WithConcurrency</span>(<span class="syn-num">8</span>),
    valka.<span class="syn-fn">WithHandler</span>(<span class="syn-kw">func</span>(ctx *valka.<span class="syn-typ">TaskContext</span>) (<span class="syn-kw">any</span>, <span class="syn-kw">error</span>) {
        <span class="syn-kw">var</span> input <span class="syn-typ">Email</span>
        <span class="syn-kw">if</span> err := ctx.<span class="syn-fn">Input</span>(&amp;input); err != <span class="syn-kw">nil</span> {
            <span class="syn-kw">return nil</span>, err
        }
        <span class="syn-kw">return</span> <span class="syn-kw">map</span>[<span class="syn-kw">string</span>]<span class="syn-kw">any</span>{<span class="syn-str">"sent"</span>: <span class="syn-kw">true</span>}, send(input)
    }),
)
<span class="syn-kw">if</span> err != <span class="syn-kw">nil</span> {
    log.<span class="syn-fn">Fatal</span>(err)
}
log.<span class="syn-fn">Fatal</span>(worker.<span class="syn-fn">Run</span>(context.<span class="syn-fn">Background</span>()))`,
};

export function SdkTabs() {
  const [active, setActive] = useState<Lang>('Rust');

  return (
    <section className="mx-auto grid max-w-6xl gap-10 px-6 pb-20 lg:grid-cols-[1fr_1.4fr]">
      <AnimateIn>
        <h2 className="text-balance text-3xl font-extrabold tracking-tight">
          Workers in four languages
        </h2>
        <p className="mt-4 text-lg leading-relaxed text-fd-muted-foreground">
          A worker lists a few nodes, takes tasks over one gRPC stream and survives any of them
          dying. Producers create tasks with the client of the same SDK, or with plain HTTP.
        </p>
        <p className="mt-4 text-sm text-fd-muted-foreground">
          Every SDK reads <code className="font-mono text-fd-foreground">VALKA_API_KEY</code> when
          the cluster requires keys.
        </p>
      </AnimateIn>

      <AnimateIn delay={0.08} className="min-w-0">
        <div className="overflow-hidden rounded-xl border border-fd-border bg-[#0d1211] shadow-sm">
          <div role="tablist" aria-label="SDK language" className="flex border-b border-white/[0.06] px-2">
            {languages.map((lang) => (
              <button
                key={lang}
                role="tab"
                aria-selected={active === lang}
                onClick={() => setActive(lang)}
                className={`relative px-3.5 py-3 text-sm font-medium transition-colors ${
                  active === lang ? 'text-[#e8eeeb]' : 'text-[#7f8c87] hover:text-[#c3ccc8]'
                }`}
              >
                {lang}
                {active === lang && (
                  <span className="absolute inset-x-3 -bottom-px h-0.5 rounded-full bg-[#67c2aa]" />
                )}
              </button>
            ))}
          </div>
          <pre className="overflow-x-auto p-5 font-mono text-[12.5px] leading-relaxed text-[#d7dfdb]">
            <code dangerouslySetInnerHTML={{ __html: snippets[active] }} />
          </pre>
          <div className="border-t border-white/[0.06] px-5 py-3 font-mono text-xs text-[#94a19c]">
            <span className="text-[#67c2aa]">$ </span>
            {install[active]}
          </div>
        </div>
      </AnimateIn>
    </section>
  );
}
