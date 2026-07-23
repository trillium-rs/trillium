import type {PrismTheme} from 'prism-react-renderer';

/**
 * Light theme — dappled forest
 *
 * Sage code paper (oklch 0.93 0.02 118) on the birch ground. Greens sit in
 * the 145–150° pine band; warmth comes from lichen-gold / bark-amber hues
 * (70–90°) rather than the old parchment browns.
 *
 * Token color hierarchy:
 *   Types / class names  →  #153c1d  deepest pine     (the nouns: Conn, Handler, Router)
 *   Builtins / attr      →  #37683a  forest green
 *   Functions            →  #3e7846  living green
 *   Keywords             →  #556252  muted gray-green  (Victor Mono italic does visual work)
 *   Strings              →  #866f2c  lichen gold
 *   Numbers / booleans   →  #966626  bark amber
 *   Operators            →  #69502e  dark amber-brown
 *   Punctuation          →  #6e6a4f  olive drab
 *   Comments             →  #878870  dry moss, italic  (whisper)
 */
export const lightTheme: PrismTheme = {
  plain: {
    color: '#233225',
    backgroundColor: '#e6eadb',
  },
  styles: [
    {
      types: ['comment', 'prolog', 'doctype', 'cdata'],
      style: {color: '#878870', fontStyle: 'italic'},
    },
    {
      types: ['namespace'],
      style: {opacity: 0.7},
    },
    {
      types: ['string', 'char', 'attr-value'],
      style: {color: '#866f2c'},
    },
    {
      types: ['punctuation'],
      style: {color: '#6e6a4f'},
    },
    {
      types: ['operator'],
      style: {color: '#69502e'},
    },
    {
      types: ['number', 'boolean', 'inserted'],
      style: {color: '#966626'},
    },
    {
      // Muted — Victor Mono cursive italic (via CSS) does the visual work
      types: ['keyword', 'atrule'],
      style: {color: '#556252', fontStyle: 'italic'},
    },
    {
      types: ['builtin', 'attr-name', 'selector'],
      style: {color: '#37683a'},
    },
    {
      types: ['function', 'function-variable', 'macro'],
      style: {color: '#3e7846'},
    },
    {
      // Types are the nouns — give them the darkest, most prominent green
      types: ['class-name', 'maybe-class-name', 'tag'],
      style: {color: '#153c1d'},
    },
    {
      types: ['constant', 'symbol', 'variable', 'regex'],
      style: {color: '#7b5e29'},
    },
    {
      types: ['deleted'],
      style: {color: '#8b3a3a'},
    },
    {
      types: ['important'],
      style: {color: '#153c1d', fontStyle: 'italic'},
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
