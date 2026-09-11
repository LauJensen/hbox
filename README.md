# Hbox

**Turns a screenshot into a website in minutes**

[![Screenshot of Hbox](docs/images/readme_screenshot.png)](docs/images/readme_screenshot.png)

On its own, `hbox` is a static site builder which lets you create, maintain and deploy websites very quickly.

Blogging is a first class citizen, which supports converting pure markdown into blogposts.

On top of that, you can import a screenshot (possibly ai generated) of a website and import this directly as a page into an existing `hbox` site, or as the starting point for a brand new site. The screenshot is converted into developer-friendly semantic HTML and CSS.

Example workflow:

``` text
* Jam with ChatGPT about website designs     (10 minutes)
* Ask ChatGPT to render a high fidelity mock (2 minutes)
* Ask hbox to import that mock               (3 minutes)
* Deploy to webserver                        (10 secs)
```

## Installation

hbox is offered as a single stand-alone executable for all platforms. Download it from our [releases page](https://github.com/LauJensen/hbox/releases).

If you have rust installed, simply run

``` bash
cargo install --git https://github.com/LauJensen/hbox --locked
```

## Configuration

hbox itself requires no configuration, but to use LLM features, you should add the following to your environment

``` bash
export OPENAI_API_KEY=MYKEY
export OPENAI_MODEL="gpt-5.6-terra"
export OPENAI_IMAGE_MODEL="gpt-image-2"
```

Pick whichever model you feel is the best balance between speed, price and quality.

## Why hbox?

Modern AI tools can generate impressive visual website concepts quickly, but getting from a mockup to a clean, editable, deployable site is still awkward and maintaining, blogging and otherwise working with the site requires constant LLM use.

`hbox` exists to close that gap.

It lets you:

* Scaffold a simple static site
* Import a screenshot or mockup as a real HTML page
* Generate image assets with AI automatically
* Edit pages locally
* Write blog posts in Markdown
* Build a deployable static site
* Keep full ownership of the resulting code
* Add repeatable/dynamic components via MiniJinja
* Validate everything before deploying

`hbox` is not trying to be Webflow, Squarespace, or a full CMS - it's much simpler and it's yours.

It is a sharp tool for developers who want speed without giving up control.

## Features

### Validation

hbox checks the following before deploy

- Are all images accessible?
- Are all links valid?
- Are all external links accessible? (run with --check-external-links)
- Are all HTML files semantically correct?
- Are all CSS files valid and accessible?

### Optimization

hbox automatically minifies and optimizes your entire site. This includes converting all images to performant versions, suitable for all devices. In some cases this reduces download size by 90% without sacrificing quality.

### Hot reloading

While working on your site, run

``` bash
hbox preview my-site
```

This opens `http://127.0.0.1:8080` and serves your site.

Every change you make is compiled and served in real-time, making updates easy and safe.

### Previewing & Staging

LLMs are inherently unpredictable. Both the `import` and `update` commands automatically do a full backup of your site before editing. When the LLM is done the result is stored in a new preview version, example

``` bash
> hbox update my-site about "Add a section for employees, add cards for Rick and Morty, with their phone numbers and email"
✓ Updated my-site, preview 1

> hbox preview my-site 1
```

If the changes look good, commit them using

``` bash
hbox accept my-site 1
```



## Project status

Hbox is close to version 1.0 and is currently hosting several high traffic sites like [lbjgruppen.com](https://www.lbjgruppen.com)

This workflow is currently fully supported

```bash
> hbox init my-site
✓ my-site initialized
> hbox import my-site screenshot.png about
✓ my-site preview 1 generated
> hbox preview my-site 1   (hot reloading while you browse)
✓ serving preview on 127.0.0.1:8080
> hbox update my-site about "Use a green color-scheme instead"
✓ my-site preview 2 generated
> hbox accept my-site 2
✓ my-site replaced by my-site-preview-2
> hbox build my-site
✓ my-site built
> rsync my-site
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

They can even use Markdown

```html
{% include "partials/header.html" %}
{% include "partials/menu.html" %}

<main>
    {% filter markdown %}
    # About us

    This page is rendered as ordinary HTML.
    {% endfilter %}
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

## Commands

### General

All commands operate in `./sites/` and `./dist/`.

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
* built-in hosting & publishing

These may be explored later, but the first version is intentionally small.

---

## Hosting

While a plan exists for some very, very excellent built-in hosting currently your best bet is nginx.

On Arch linux, install NGINX and ACME/LetsEncrypt:

``` bash
sudo pacman -S nginx nginx-mod-acme
```

For a maximum throughput configuration, have a look in `resources/nginx/nginx.conf`. Make sure to replace <YOUR EMAIL> with the actual email you want sent to LetsEncrypt for SSL cert generation.

Each hbox site that you want to host, needs the following in its hbox.toml

``` toml
[nginx]
domains = ["foo.com", "www.foo.com"]
access_log = true
```

If domains are set, `my-site/nginx.conf` will be emitted on build and the nginx.conf knows to look for it.

Once `/etx/nginx/nginx.conf` and potentially `/etc/nginx/sites-enabled/00-default.conf` you can simply rsync hbox sites into `/src/hbox`. After the very first upload, you must manually run `systemctl reload nginx`.

For easy deployment, you can use these permissions, where <deploy> is whichever user account you ssh into.

``` bash
mkdir -p /srv/hbox
chown deploy:deploy /srv/hbox
chmod 755 /srv/hbox
```

## License

Hbox is licensed under the [MIT License](LICENSE).

## Credits

The following developers have contributed to hbox:

- Lau B. Jensen @ lbjgruppen.com
- You ?
