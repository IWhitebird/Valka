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
        fontFamily: 'Inter, system-ui, sans-serif',
        theme: isDark ? 'dark' : 'default',
        themeVariables: isDark
          ? {
              primaryColor: '#818cf8',
              primaryTextColor: '#fafafa',
              primaryBorderColor: '#4f46e5',
              lineColor: '#52525b',
              secondaryColor: '#111111',
              tertiaryColor: '#0a0a0a',
              background: '#000000',
              mainBkg: '#111111',
              nodeBorder: '#3f3f46',
              clusterBkg: '#0a0a0a',
              clusterBorder: '#27272a',
              titleColor: '#fafafa',
              edgeLabelBackground: '#0a0a0a',
              noteTextColor: '#a1a1aa',
              noteBkgColor: '#111111',
              noteBorderColor: '#27272a',
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
