# Hbox

**Turns a screenshot into a website in minutes**

[![Screenshot of Hbox](docs/images/readme_screenshot.png)](docs/images/readme_screenshot.png)

On its own, `hbox` is a static site builder which lets you create, extend and deploy websites very quickly.

Blogging is a first class citizen, which supports converting pure markdown into blogposts.

On top of that, you can import a screenshot (possibly ai generated) of a website and import this directly as a page into an existing `hbox` site, or as the starting point for a brand new site.

Example

``` text
* Jam with ChatGPT about website designs     (10 minutes)
* Ask ChatGPT to render a high fidelity mock (2 minutes)
* Ask hbox to import that mock               (3 minutes)
* Deploy to webserver                        (10 secs)
```

---

## Why hbox?

Modern AI tools can generate impressive visual website concepts quickly, but getting from a mockup to a clean, editable, deployable site is still awkward.

`hbox` exists to close that gap.

It lets you:

* scaffold a simple static site
* import a screenshot or mockup as a real HTML page
* generate image assets with AI
* edit pages locally
* write blog posts in Markdown
* build a deployable static site
* keep full ownership of the resulting code
* add repeatable/dynamic components via MiniJinja

`hbox` is not trying to be Webflow, Squarespace, or a full CMS - it's much simpler and it's yours.

It is a sharp tool for developers who want speed without giving up control.

---

## Project status

Hbox is close to version 1.0 and is currently hosting several high traffic sites like [lbjgruppen.com](https://www.lbjgruppen.com)

This workflow is currently fully supported

```bash
hbox init my-site
hbox import my-site screenshot.png about
hbox preview my-site 1   (hot reloading while you browse)
hbox accept my-site 1
hbox build my-site
rsync my-site
```

The architecture is intentionally simple and may still change while the project matures.

---

## Core concept

An `hbox` site is just a folder of static files:

```text
sites/my-site/
  hbox.toml
  global.css

  pages/
    index.html
    about.html
    contact.html

  partials/
    header.html
    menu.html
    footer.html

  templates/
    blogpost.html
    blogpost-video.html

  blogposts/
    hello-world.md

  public/
    images/
      hero.png
```

### Pages are pages

Regular website pages live in `pages/` as normal HTML files:

```text
pages/index.html   -> /en/
pages/about.html   -> /en/about/
pages/contact.html -> /en/contact/
```

No JSON, No database, just plain html files.

### Partials are shared fragments

Only shared site-wide elements live in `partials/`:

```text
partials/header.html
partials/footer.html
```

Pages can include them using MiniJinja:

```html
{% include "partials/header.html" %}
{% include "partials/menu.html" %}

<main>
  <h1>About us</h1>
  <p>This page is ordinary editable HTML.</p>
</main>

{% include "partials/footer.html" %}
```

### Templates are for generated content types

The `templates/` folder is reserved for content that needs rendering, such as blog posts:

```text
templates/blogpost.html
templates/blogpost-video.html
```

A minimal blog post template might be:

```html
{% include "partials/header.html" %}

<main class="blogpost">
  {{ content }}
</main>

{% include "partials/footer.html" %}
```

### Blog posts are Markdown

Blog content lives in Markdown:

```text
/blogposts/my-first-post.md
```

The contents are pure markdown with a small header. The following example shows all options in the header, but not all are mandatory:

```markdown
---
title:       "Flocking Quadtrees"
slug:        "flocking-quadtrees"
language:    "en"
description: "Learn how to make a flocking simulation using Quadtrees and Clojurescript. It's a revisit of last weeks post about Functional Quadtrees."
date:        "2025-12-15"
image:       "/blogposts/flocking_quadtrees.png"
template:    "templates/blogpost.html"
externals:
  - https://cdn.jsdelivr.net/gh/LauJensen/practical-quadtree@master/public/js/main.js
---

# This is the first header on the page

This is the **actual** blog post content.
```

---

## Layout classes

Hbox includes a small set of design-neutral layout classes in `global.css`.

Use `.container` to constrain page width, `.section` for vertical page sections, and `.grid-*` or `.grid-auto` for tiled layouts.

For smaller layout groups, use `.row` and `.column` for flex layouts, `.stack` for vertical spacing, and `.cluster` for wrapping horizontal groups such as navigation, tags, or buttons.

Common structural patterns are also available:

* `.card` for equal-height card layouts
* `.sidebar-layout` for sidebar and content layouts
* `.frame-*` for responsive image and video ratios
* `.overlap` for layered content
* `.reel` for horizontally scrolling items

These classes handle structure only. Fonts, colors, borders, shadows, and other visual styling belong in `design.css`. Page-specific exceptions belong in the page's own CSS file.


---

## Installation

TBD

---

## Commands

### General

All commands work in `./sites/` and `./dist/`.

The `sites` folder contains your source-files for each site. These are human-readable unoptimized html, css and md files.

The `dist`folder contains the sites which you deploy to your webserver, they are highly optimized and not necessarily readable by humans.

### `hbox init`

Create a new Hbox site.

```bash
hbox init my-site
```

Example:

```bash
hbox init lbjgruppen.com
```

---

### `hbox import`

Import a screenshot or mockup and turn it into a static HTML page.

```bash
hbox import lbjgruppen.com screenshot.png about
```

This generates:

```text
sites/lbjgruppen.com/pages/about.html
sites/lbjgruppen.com/public/images/...
```

If shared partials are missing, Hbox can generate:

```text
sites/my-site/partials/header.html
sites/my-site/partials/footer.html
```

If the partials already exist, the generated page reuses them instead of recreating site chrome.

---

TBD

Hbox does not try to hide the result behind a page builder abstraction. After import, you can open the files and edit them directly.

---

## Design principles

### 1. Own the code

Generated output should be real files developers can inspect, edit, commit, and deploy.

### 2. Keep pages simple

Pages are HTML files in `pages/`.

No page JSON.
No hidden CMS records.
No unnecessary abstraction.

### 3. Use MiniJinja only where it earns its keep

MiniJinja is useful for:

* shared partials
* blog templates
* navigation/menu reuse
* loops and generated listings

It is not required for ordinary one-off page text.

### 4. Make AI useful, not magical

AI should accelerate the first draft and snappy updates, not trap the developer in generated complexity.

### 5. Optimize for the first five minutes

The primary experience should be:

```text
I have a screenshot.
I run one command.
I get a real page.
I can edit it.
I can build it.
I can put it online.
```

---

## Example workflow

TBD

---

## Current MVP scope

In scope for v1:

* static page building
* screenshot-to-page import
* shared header/footer partials
* global CSS and design CSS
* generated image assets
* Markdown blog posts
* local preview server with hot-reloading
* simple deploy/sync flow

Out of scope for v1:

* full CMS
* block editor
* theme marketplace
* visual layout builder
* user accounts
* multi-user editing
* complex content schemas
* generalized component system

These may be explored later, but the first version is intentionally small.

---


## License

TBD.

## Credits

The following developers have contributed to hbox:

- Lau B. Jensen @ lbjgruppen.com
- You ?
