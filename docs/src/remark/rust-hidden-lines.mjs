// Remark plugin that strips rustdoc-style hidden lines from rust code blocks
// before rendering. Lines beginning with "# " and bare "#" lines are removed,
// matching the convention used by the docs-check script.
//
// This keeps the hidden lines in the source markdown (where they provide
// compilation context) while keeping them out of the rendered output.

/** @returns {import('unified').Transformer} */
export default function remarkRustHiddenLines() {
  return (tree) => {
    visitCode(tree);
  };
}

function visitCode(node) {
  if (node.type === 'code' && node.lang === 'rust') {
    node.value = node.value
      .split('\n')
      .filter(line => line !== '#' && !line.startsWith('# '))
      .join('\n');
  }
  if (node.children) {
    for (const child of node.children) visitCode(child);
  }
}
