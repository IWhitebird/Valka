import type { BaseLayoutProps } from 'fumadocs-ui/layouts/shared';

export function baseOptions(): BaseLayoutProps {
  return {
    nav: {
      title: (
        <div className="flex items-center gap-2.5">
          <img src="/logo.svg" alt="" className="size-6" />
          <span className="font-display text-lg font-extrabold tracking-tight">valka</span>
        </div>
      ),
      url: '/',
    },
    links: [
      { text: 'Docs', url: '/docs', active: 'nested-url' },
      {
        text: 'GitHub',
        url: 'https://github.com/IWhitebird/Valka',
        external: true,
      },
    ],
    githubUrl: 'https://github.com/IWhitebird/Valka',
  };
}
