# Hbox

**Turns a screenshot into a website in minutes**

[![Screenshot of Hbox](docs/images/readme_screenshot.png)](docs/images/readme_screenshot.png)

On its own, `hbox` is a static site builder which lets you create, maintain and deploy websites very quickly.

Blogging is a first class citizen, which supports converting pure markdown into blogposts.

Building, validating and optimize (90% size reduction!), takes only seconds:
[![hbox workflow](docs/images/build_validate_optimize.gif)](docs/images/build_validate_optimize.gif)

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
export OPENAI_MODEL="gpt-5.1"
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

hbox can check the following before deploy

- Do images have sources and alt attributes?
- Do local links, images, resources and fragments exist?
- Are HTML language attributes present?
- Are resources referenced from CSS accessible?
- Are external links accessible? (run with `--check-external-links`)

### Optimization

hbox automatically minifies your CSS during builds. Running `hbox optimize my-site` afterwards converts referenced local images to WebP, creates responsive variants and rewrites image tags with `srcset`. In some cases this reduces download size by 90% without sacrificing quality.

### Hot reloading

While working on your site, run

``` bash
hbox preview my-site
```

This opens `http://127.0.0.1:8080` and serves your site.

If built output already exists, hbox serves it without rebuilding first, so an optimized output directory is preserved. If it does not exist, hbox builds it automatically.

Every change you make is compiled and served in real-time, making updates easy and safe.

### Previewing & Staging

LLMs are inherently unpredictable. Both the `import` and `update` commands leave your accepted site untouched and apply their changes to a complete preview copy. When the LLM is done the result is stored in a new preview version, example

``` bash
> hbox update my-site about "Add a section for employees, add cards for Rick and Morty, with their phone numbers and email"
✓ Updated my-site, preview 1
> hbox preview my-site 1
```

If the changes look good, commit them using

``` bash
> hbox accept my-site 1
```



## Project status

Hbox is close to version 1.0 and is currently hosting several high traffic sites like [lbjgruppen.com](https://www.lbjgruppen.com)

This workflow is currently fully supported

```bash
hbox init my-site
hbox import my-site screenshot.png about
hbox preview my-site 1   # Inspect, then stop with Ctrl+C
hbox accept my-site 1
hbox update my-site about "Use a green color-scheme instead"
hbox preview my-site 1   # Inspect, then stop with Ctrl+C
hbox accept my-site 1
hbox build my-site
hbox optimize my-site
rsync -az --delete dist/my-site/ deploy@example.com:/srv/hbox/my-site/
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
pages/index.html   -> /
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
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <title>My awesome site</title>
  </head>
  <body>
    {% include "partials/header.html" %}

    <div class="blogpost-content">
      {{ content }}
    </div>

    {% include "partials/footer.html" %}
  </body>
</html>
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
draft:       false
code_theme:  "ocean-dark"
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

The `dist` folder contains the generated sites which you deploy to your webserver. CSS is minified during builds, while image optimization is performed separately by `hbox optimize`.

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

The third argument is the required page slug.

This generates a numbered preview, for example:

```text
sites/.preview-lbjgruppen.com-1/pages/about.html
sites/.preview-lbjgruppen.com-1/public/images/...
dist/.preview-lbjgruppen.com-1/...
```

If shared partials are missing, Hbox can generate:

```text
sites/.preview-my-site-1/partials/header.html
sites/.preview-my-site-1/partials/footer.html
```

If the partials already exist, the generated page reuses them instead of recreating site chrome.

---

Hbox does not try to hide the result behind a page builder abstraction. After import, you can open the preview files and edit them directly, or accept the preview to make them the site's source files.

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

If domains are set, `dist/my-site/nginx.conf` will be emitted on build and the main nginx.conf knows to look for it.

Once `/etc/nginx/nginx.conf` and potentially `/etc/nginx/sites-enabled/00_default.conf` are installed, you can rsync built hbox sites from `dist/` into `/srv/hbox`. After the very first upload, you must manually run `systemctl reload nginx`.

For easy deployment, you can use these permissions, where <deploy> is whichever user account you ssh into.

``` bash
sudo install -d -o deploy -g deploy -m 755 /srv/hbox
```

The deploy account can then upload a built site without root access:

``` bash
rsync -az --delete dist/my-site/ deploy@example.com:/srv/hbox/my-site/
```

You can take a shortcut and achieve the same, by adding this to your sites `hbox.toml`:

``` toml
[deployment]
ssh_user = "deploy"
ssh_host = "example.com"
deploy_path = "/srv/hbox"
```

Notice the `deploy_path` is ready to host multiple sites and will install yours in `/srv/hbox/your-site`.

To deploy, simply run

``` bash
hbox deploy my-site
```

## License

Hbox is licensed under the [MIT License](LICENSE).

## Credits

The following developers have contributed to hbox:

- Lau B. Jensen @ lbjgruppen.com
- You ?
