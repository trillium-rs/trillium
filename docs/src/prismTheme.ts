import type {PrismTheme} from 'prism-react-renderer';

/**
 * Light theme — forest floor
 *
 * Token color hierarchy (light mode, parchment bg):
 *   Types / class names  →  #1b4332  darkest green   (the nouns: Conn, Handler, Router)
 *   Builtins / attr      →  #2d6a4f  forest green
 *   Functions            →  #40916c  medium green
 *   Keywords             →  #607060  muted gray-green  (Victor Mono italic does visual work)
 *   Strings              →  #7a5c2e  warm amber-brown
 *   Numbers / booleans   →  #8b5e3c  terracotta
 *   Operators            →  #5a3e1e  dark brown
 *   Punctuation          →  #7a6b55  medium warm brown
 *   Comments             →  #9a8060  taupe, italic     (whisper)
 */
export const lightTheme: PrismTheme = {
  plain: {
    color: '#2a1a0a',
    backgroundColor: '#f0e8d8',
  },
  styles: [
    {
      types: ['comment', 'prolog', 'doctype', 'cdata'],
      style: {color: '#9a8060', fontStyle: 'italic'},
    },
    {
      types: ['namespace'],
      style: {opacity: 0.7},
    },
    {
      types: ['string', 'char', 'attr-value'],
      style: {color: '#7a5c2e'},
    },
    {
      types: ['punctuation'],
      style: {color: '#7a6b55'},
    },
    {
      types: ['operator'],
      style: {color: '#5a3e1e'},
    },
    {
      types: ['number', 'boolean', 'inserted'],
      style: {color: '#8b5e3c'},
    },
    {
      // Muted — Victor Mono cursive italic (via CSS) does the visual work
      types: ['keyword', 'atrule'],
      style: {color: '#607060', fontStyle: 'italic'},
    },
    {
      types: ['builtin', 'attr-name', 'selector'],
      style: {color: '#2d6a4f'},
    },
    {
      types: ['function', 'function-variable', 'macro'],
      style: {color: '#40916c'},
    },
    {
      // Types are the nouns — give them the darkest, most prominent green
      types: ['class-name', 'maybe-class-name', 'tag'],
      style: {color: '#1b4332'},
    },
    {
      types: ['constant', 'symbol', 'variable', 'regex'],
      style: {color: '#6b4c2a'},
    },
    {
      types: ['deleted'],
      style: {color: '#8b3a3a'},
    },
    {
      types: ['important'],
      style: {color: '#1b4332', fontStyle: 'italic'},
    },
  ],
};

/**
 * Dark theme — forest night
 *
 * Token color hierarchy (dark mode, dark-green bg):
 *   Types / class names  →  #9dd4a0  light forest green  (the nouns, most prominent)
 *   Functions            →  #7bbf80  medium forest green
 *   Builtins / attr      →  #5ea864  medium-dark forest green
 *   Keywords             →  #6a9670  muted forest green  (Victor Mono italic does visual work)
 *   Strings              →  #e9a84c  warm amber
 *   Numbers / booleans   →  #f0c070  warm gold
 *   Operators            →  #c8b89a  warm off-white
 *   Punctuation          →  #a09070  warm tan
 *   Comments             →  #7a6a55  muted brown, italic (whisper)
 */
export const darkTheme: PrismTheme = {
  plain: {
    color: '#f0e6d8',
    backgroundColor: '#0d1a10',
  },
  styles: [
    {
      types: ['comment', 'prolog', 'doctype', 'cdata'],
      style: {color: '#7a6a55', fontStyle: 'italic'},
    },
    {
      types: ['namespace'],
      style: {opacity: 0.7},
    },
    {
      types: ['string', 'char', 'attr-value'],
      style: {color: '#e9a84c'},
    },
    {
      types: ['punctuation'],
      style: {color: '#a09070'},
    },
    {
      types: ['operator'],
      style: {color: '#c8b89a'},
    },
    {
      types: ['number', 'boolean', 'inserted'],
      style: {color: '#f0c070'},
    },
    {
      // Muted — Victor Mono cursive italic (via CSS) does the visual work
      types: ['keyword', 'atrule'],
      style: {color: '#6a9670', fontStyle: 'italic'},
    },
    {
      types: ['builtin', 'attr-name', 'selector'],
      style: {color: '#5ea864'},
    },
    {
      types: ['function', 'function-variable', 'macro'],
      style: {color: '#7bbf80'},
    },
    {
      // Types are the nouns — brightest, most prominent green
      types: ['class-name', 'maybe-class-name', 'tag'],
      style: {color: '#9dd4a0'},
    },
    {
      types: ['constant', 'symbol', 'variable', 'regex'],
      style: {color: '#d4a76a'},
    },
    {
      types: ['deleted'],
      style: {color: '#e06c75'},
    },
    {
      types: ['important'],
      style: {color: '#9dd4a0', fontStyle: 'italic'},
    },
  ],
};
