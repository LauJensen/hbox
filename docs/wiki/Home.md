# Hbox

Hbox is a small static-site builder designed for AI-generated source files. It
turns screenshots and natural-language changes into ordinary HTML, CSS,
Markdown, and local assets that remain easy to inspect and edit by hand.

Hbox provides:

- screenshot-to-page imports;
- natural-language page updates;
- complete, numbered previews before changes are accepted;
- MiniJinja pages, templates, and reusable partials;
- Markdown blogging with syntax highlighting;
- minified, content-hashed production CSS;
- image optimization and link validation; and
- directory-style static output with language-aware routes.

## Quick start

Install Hbox from a local checkout:

```sh
cargo install --path .
```

Create and serve a site:

```sh
hbox init example.com
hbox serve example.com
```

Hbox keeps source sites under `sites/` and writes production output under
`dist/`. The argument `example.com` therefore resolves to
`sites/example.com/`, while its build is written to `dist/example.com/`.

## Site structure

```text
sites/example.com/
├── hbox.toml
├── global.css
├── design.css
├── pages/
│   ├── index.html
│   ├── about.html
│   └── about.css
├── partials/
│   ├── header.html
│   └── footer.html
├── templates/
│   └── blogpost.html
├── blogposts/
│   └── hello-hbox.md
└── public/
    └── images/
```

The important distinction is between source and output:

- `sites/<name>/` is yours to edit and commit.
- `dist/<name>/` is generated and may be replaced on every build.
- `sites/<name>-previewN/` contains a complete proposed version created by an
  import or update.

## Pages and routes

Every page is a complete HTML document and must contain an `<html lang="...">`
element and a closing `</head>` tag.

- `pages/index.html` becomes `/index.html`.
- `pages/about.html` with `lang="en"` becomes `/en/about/index.html`.
- `pages/docs/install.html` with `lang="en"` becomes
  `/en/docs/install/index.html`.

Page files are MiniJinja templates. They can include shared partials:

```jinja
{% include "partials/header.html" %}
```

They can also contain small Markdown-authored regions:

```jinja
{% filter markdown %}
## A boringly good file tree

Write ordinary **Markdown** here.
{% endfilter %}
```

## CSS ownership

Hbox divides CSS into three layers:

| Source | Purpose | Build behavior |
| --- | --- | --- |
| `global.css` | Stable site-wide elements, typography, layout, and reusable components | Minified and emitted as `/global.css` |
| `design.css` | The site's visual identity: colors, fonts, surfaces, and decorative styling | Minified and emitted with a content hash |
| `pages/<page>.css` | Rules genuinely specific to one page | Minified and inlined into that page |

Do not add `<link>` elements for `global.css` or `design.css` yourself. Hbox
injects them during the build.

## Documentation

- [Configuration](Configuration.md)
- [Commands](Commands.md)
- [Blogging](Blogging.md)

