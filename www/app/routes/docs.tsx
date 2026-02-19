import type { Route } from './+types/docs';
import { DocsLayout } from 'fumadocs-ui/layouts/docs';
import {
  DocsBody,
  DocsDescription,
  DocsPage,
  DocsTitle,
} from 'fumadocs-ui/layouts/docs/page';
import { source } from '@/lib/source';
import defaultMdxComponents from 'fumadocs-ui/mdx';
import browserCollections from 'fumadocs-mdx:collections/browser';
import { baseOptions } from '@/lib/layout.shared';
import { useFumadocsLoader } from 'fumadocs-core/source/client';
import { Mermaid } from '@/components/mermaid';

export async function loader({ params }: Route.LoaderArgs) {
  const slugs = params['*'].split('/').filter((v) => v.length > 0);
  const page = source.getPage(slugs);
  if (!page) throw new Response('Not found', { status: 404 });

  return {
    path: page.path,
    url: page.url,
    pageTree: await source.serializePageTree(source.getPageTree()),
  };
}

const clientLoader = browserCollections.docs.createClientLoader({
  component({ toc, frontmatter, default: Mdx }) {
    return (
      <DocsPage
        toc={toc}
        tableOfContent={{
          style: 'clerk',
        }}
      >
        <title>{`${frontmatter.title} — Valka`}</title>
        <meta name="description" content={frontmatter.description} />
        <DocsTitle>{frontmatter.title}</DocsTitle>
        <DocsDescription className="text-fd-muted-foreground">
          {frontmatter.description}
        </DocsDescription>
        <DocsBody>
          <Mdx components={{ ...defaultMdxComponents, Mermaid }} />
        </DocsBody>
      </DocsPage>
    );
  },
});

export default function Page({ loaderData }: Route.ComponentProps) {
  const { path, pageTree } = useFumadocsLoader(loaderData);

  return (
    <DocsLayout
      {...baseOptions()}
      tree={pageTree}
      sidebar={{
        defaultOpenLevel: 1,
        collapsible: true,
      }}
    >
      {clientLoader.useContent(path)}
    </DocsLayout>
  );
}
