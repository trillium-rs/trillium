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
        'overview/runtimes',
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
        'handlers/http_client',
        'handlers/proxy',
        'handlers/utilities',
      ],
    },
    'testing',
    'library_patterns',
    'contributing',
  ],
};

export default sidebars;
