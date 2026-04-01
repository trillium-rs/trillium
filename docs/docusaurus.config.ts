import type {Config} from '@docusaurus/types';
import type * as Preset from '@docusaurus/preset-classic';
import remarkRustHiddenLines from './src/remark/rust-hidden-lines.mjs';
import {lightTheme, darkTheme} from './src/prismTheme';

const config: Config = {
  title: 'trillium',
  tagline: 'Composition as Configuration',
  url: 'https://trillium.rs',
  baseUrl: '/',
  organizationName: 'trillium-rs',
  projectName: 'trillium',

  onBrokenLinks: 'throw',
  markdown: {
    hooks: {
      onBrokenMarkdownLinks: 'warn',
    },
  },

  i18n: {
    defaultLocale: 'en',
    locales: ['en'],
  },

  presets: [
    [
      'classic',
      {
        docs: {
          path: 'guide',
          routeBasePath: 'guide',
          sidebarPath: './sidebars.ts',
          remarkPlugins: [remarkRustHiddenLines],
          editUrl:
            'https://github.com/trillium-rs/trillium/edit/main/docs/',
          lastVersion: 'current',
          versions: {
            current: {
              label: '1.0',
            },
            '0.2': {
              label: '0.2',
            },
          },
        },
        blog: {
          showReadingTime: true,
          feedOptions: {
            type: ['rss', 'atom'],
            xslt: true,
          },
          editUrl:
            'https://github.com/trillium-rs/trillium/edit/main/docs/',
          onInlineTags: 'warn',
          onInlineAuthors: 'warn',
          onUntruncatedBlogPosts: 'warn',
        },
        theme: {
          customCss: './src/css/custom.css',
        },
      } satisfies Preset.Options,
    ],
  ],

  themeConfig: {
    colorMode: {
      respectPrefersColorScheme: true,
    },
    navbar: {
      title: 'trillium',
      items: [
        {
          type: 'docSidebar',
          sidebarId: 'guideSidebar',
          position: 'left',
          label: 'Guide',
        },
        {to: '/blog', label: 'Blog', position: 'left'},
        {
          type: 'docsVersionDropdown',
          position: 'right',
        },
        {
          href: 'https://docs.trillium.rs',
          label: 'API Docs',
          position: 'right',
        },
        {
          href: 'https://github.com/trillium-rs/trillium',
          label: 'GitHub',
          position: 'right',
        },
      ],
    },
    footer: {
      style: 'dark',
      links: [
        {
          title: 'Learn',
          items: [
            {label: 'Guide', to: '/guide/welcome'},
            {label: 'API Docs', href: 'https://docs.trillium.rs'},
          ],
        },
        {
          title: 'Crates',
          items: [
            {label: 'crates.io', href: 'https://crates.io/crates/trillium'},
            {label: 'docs.rs', href: 'https://docs.rs/trillium'},
          ],
        },
        {
          title: 'Community',
          items: [
            {
              label: 'GitHub',
              href: 'https://github.com/trillium-rs/trillium',
            },
            {label: 'Blog', to: '/blog'},
          ],
        },
      ],
      copyright: `Copyright © ${new Date().getFullYear()} Jacob Rothstein. Built with Docusaurus.`,
    },
    prism: {
      theme: lightTheme,
      darkTheme: darkTheme,
      additionalLanguages: ['rust', 'toml', 'bash'],
    },
  } satisfies Preset.ThemeConfig,
};

export default config;
