# Configuration

Each site is configured by `sites/<site>/hbox.toml`.

## Example

```toml
[site]
name = "Example"
base_url = "https://example.com"
default_language = "en"
date_format = "%d.%m.%Y"
default_title = "Example"
code_theme = "ocean-dark"
```

## Site settings

| Setting | Required | Description |
| --- | --- | --- |
| `name` | Yes | Human-readable site name exposed to templates as `site.name`. |
| `base_url` | No | Canonical public origin, without a trailing path. |
| `default_language` | No | Language used when a blog post omits `language`. |
| `date_format` | No | Chrono/strftime format used when dates are exposed to templates. For example, `%d.%m.%Y` produces `06.09.2026`. |
| `default_title` | No | Site-wide fallback title available to templates. |
| `code_theme` | No | Default syntax-highlighting theme for fenced code blocks. |

Available code themes are:

- `github`
- `solarized-dark`
- `solarized-light`
- `eighties-dark`
- `mocha-dark`
- `ocean-dark`
- `ocean-light`

A blog post can override the site-wide theme in its front matter.

## Environment variables

AI-backed commands read their OpenAI configuration from the environment:

| Variable               | Purpose                                                                                 |
|------------------------|-----------------------------------------------------------------------------------------|
| `OPENAI_API_KEY`       | API key used by `import` and `update`.                                                  |
| `OPENAI_MODEL`         | Text model used to generate HTML, CSS, SVG, and structured changes.                     |
| `OPENAI_IMAGE_MODEL`   | Model used to generate raster image assets.                                             |
| `OPENAI_IMAGE_QUALITY` | Quality passed to image generation.                                                     |
| `OPENAI_BASE_URL`      | Optional alternative API base URL, primarily useful for testing or compatible gateways. |

For a shell session:

```sh
export OPENAI_API_KEY="..."
export OPENAI_MODEL="gpt-5.1"
export OPENAI_IMAGE_MODEL="gpt-image-2"
```

Do not commit API keys. Put local values in your shell environment or another
secret-management mechanism.

## Template context

Normal HTML pages receive:

- `site`: the `[site]` configuration;
- `posts`: published blog-post summaries, newest dated posts first.

Blog templates additionally receive:

- `page`: all normalized metadata for the current post;
- `title`, `description`, `language`, `slug`, and `url`;
- `content`: the rendered Markdown body, already safe to insert as HTML; and
- `posts`: the same published-post summaries available to pages.

Each item in `posts` contains `title`, `slug`, `language`, `description`, `url`,
`date`, `featured_image`, and `code_theme`.

## Managed files

Hbox owns the build treatment of these files:

- `global.css` is required and emitted under that exact name.
- Non-empty `design.css` is minified and emitted as
  `design.<content-hash>.css`.
- A sibling stylesheet such as `pages/about.css` is minified and inlined into
  the corresponding HTML page.
- Everything under `public/` is copied to the output root.

Source HTML must not reference `global.css`, `design.css`, or a generated
`design.<hash>.css` directly. Hbox inserts the correct links after the final
filenames are known.
