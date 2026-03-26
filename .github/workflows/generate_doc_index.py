import json, subprocess, os, glob

meta = json.loads(subprocess.check_output(
    ['cargo', 'metadata', '--no-deps', '--format-version', '1']
))
pkgs = sorted(p['name'] for p in meta['packages'])

def find(pattern):
    return os.path.basename(glob.glob(f'target/doc/static.files/{pattern}')[0])

normalize = find('normalize-*.css')
rustdoc   = find('rustdoc-*.css')
main_js   = find('main-*.js')
storage   = find('storage-*.js')

items = '\n'.join(
    f'<li><a href="{p.replace("-","_")}/index.html">{p}</a></li>'
    for p in pkgs
)

html = f"""<!DOCTYPE html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <title>Trillium — Index of crates</title>
  <link rel="stylesheet" href="./static.files/{normalize}">
  <link rel="stylesheet" href="./static.files/{rustdoc}">
  <script src="./static.files/{storage}"></script>
  <script defer src="./static.files/{main_js}"></script>
</head>
<body class="rustdoc mod">
  <main>
    <div class="width-limiter">
      <section id="main-content" class="content">
        <div class="main-heading"><h1>Trillium crates</h1></div>
        <ul class="all-items">{items}</ul>
      </section>
    </div>
  </main>
</body>
</html>"""

with open('target/doc/index.html', 'w') as f:
    f.write(html)
