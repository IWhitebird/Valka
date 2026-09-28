'use client';

import { useEffect, useId, useRef, useState } from 'react';

export function Mermaid({ chart }: { chart: string }) {
  const id = useId().replace(/:/g, '_');
  const containerRef = useRef<HTMLDivElement>(null);
  const [svg, setSvg] = useState<string>('');

  useEffect(() => {
    let cancelled = false;

    async function render() {
      const { default: mermaid } = await import('mermaid');

      const isDark =
        document.documentElement.classList.contains('dark') ||
        document.documentElement.getAttribute('data-theme') === 'dark';

      mermaid.initialize({
        startOnLoad: false,
        securityLevel: 'loose',
        fontFamily: 'var(--font-instrument-sans), system-ui, sans-serif',
        theme: isDark ? 'dark' : 'default',
        themeVariables: isDark
          ? {
              primaryColor: '#1f5b4f',
              primaryTextColor: '#eef2ef',
              primaryBorderColor: '#3e9f88',
              lineColor: '#56635e',
              secondaryColor: '#1b2422',
              tertiaryColor: '#141b19',
              background: '#0d1211',
              mainBkg: '#1b2422',
              nodeBorder: '#3a4642',
              clusterBkg: '#141b19',
              clusterBorder: '#25302c',
              titleColor: '#eef2ef',
              edgeLabelBackground: '#141b19',
              noteTextColor: '#94a19c',
              noteBkgColor: '#1b2422',
              noteBorderColor: '#25302c',
            }
          : {},
      });

      try {
        const { svg: rendered } = await mermaid.render(`mermaid-${id}`, chart);
        if (!cancelled) setSvg(rendered);
      } catch {
        // Mermaid render error — show raw chart
        if (!cancelled) setSvg('');
      }
    }

    render();
    return () => {
      cancelled = true;
    };
  }, [chart, id]);

  if (!svg) {
    return (
      <div className="my-6 rounded-xl border border-fd-border bg-fd-card p-6">
        <pre className="text-sm text-fd-muted-foreground">
          <code>{chart}</code>
        </pre>
      </div>
    );
  }

  return (
    <div
      ref={containerRef}
      className="mermaid-diagram my-6 overflow-x-auto rounded-xl border border-fd-border bg-fd-card p-6"
      dangerouslySetInnerHTML={{ __html: svg }}
    />
  );
}
