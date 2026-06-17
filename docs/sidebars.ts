import type {SidebarsConfig} from '@docusaurus/plugin-content-docs';

const sidebars: SidebarsConfig = {
  guideSidebar: [
    'welcome',
    'conventions',
    {
      type: 'category',
      label: 'Core Concepts',
      items: [
        'overview/handlers',
        'overview/conn',
      ],
    },
    {
      type: 'category',
      label: 'Serving',
      items: [
        'overview/runtimes',
        'overview/listeners',
        'overview/graceful-shutdown',
        'overview/tls',
        'overview/http2',
        'overview/http3',
      ],
    },
    {
      type: 'category',
      label: 'Handler Libraries',
      link: {type: 'doc', id: 'handlers'},
      items: [
        'handlers/router',
        'handlers/api',
        'handlers/logger',
        'handlers/cookies',
        'handlers/sessions',
        'handlers/static',
        'handlers/templates',
        'handlers/sse',
        'handlers/websockets',
        'handlers/channels',
        'handlers/webtransport',
        'handlers/proxy',
        'handlers/utilities',
      ],
    },
    {
      type: 'category',
      label: 'HTTP Client',
      link: {type: 'doc', id: 'client/overview'},
      items: [
        'client/middleware',
        'client/encrypted-dns',
        'client/extras',
      ],
    },
    'testing',
    'library_patterns',
    'contributing',
  ],
};

export default sidebars;
