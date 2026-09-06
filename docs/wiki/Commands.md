# Commands

Run `hbox --help` for the command list and `hbox <command> --help` for the
arguments supported by the installed version.

## `init`

Create a new site with Hbox's starter files:

```sh
hbox init example.com
```

The site is created at `sites/example.com/`. Use `--force` to replace starter
files that already exist:

```sh
hbox init example.com --force
```

## `build`

Build a site into `dist/<site>`:

```sh
hbox build example.com
```

Hbox renders pages and blog posts, copies public assets, minifies CSS, and only
publishes the completed staging build after every required artifact succeeds.
An unsuccessful build therefore leaves the last published output intact.

## `serve`

Build the site, serve it locally, and rebuild when source files change:

```sh
hbox serve example.com
hbox serve example.com --port 3000
```

The default port is `8080`. Validation findings are quality feedback; a site
that builds successfully can still be served even when the validator reports
broken links or missing fragments.

## `import`

Generate a page from an inspirational screenshot:

```sh
hbox import example.com screenshot.png
hbox import example.com screenshot.png about
hbox import example.com screenshot.png about --threads 4
```

The positional arguments are the site, screenshot, and optional page slug. The
slug defaults to `index`.

Import works on a complete numbered preview such as
`sites/example.com-preview1/`; it does not modify the accepted site directly.
The generated preview is built before Hbox reports success. Incomplete preview
and transient build artifacts are removed if generation or building fails.

## `update`

Update an existing page from a natural-language request:

```sh
hbox update example.com index "make the hero quieter"
hbox update example.com about "add a contact section" --threads 4
```

The positional arguments are the site, page slug, and prompt. Like `import`,
`update` creates and builds a complete numbered preview while leaving the
accepted site unchanged.

## `accept`

Replace the accepted site with one numbered preview:

```sh
hbox accept example.com 2
```

Hbox verifies that the requested preview exists and can be built, promotes it
to `sites/example.com/`, and removes the remaining previews for that site.

## `optimize`

Optimize an already buildable site, including supported image conversion and
responsive image output:

```sh
hbox optimize example.com
```

Optimization is performed through staging so the published output is replaced
only after the optimized result succeeds.

## `validate`

Validate generated pages, local links, images, and fragments:

```sh
hbox validate example.com
```

External HTTP checks are opt-in because they require network requests and are
slower:

```sh
hbox validate example.com --check-external-links
```

Validation is advisory during interactive serve/watch workflows, but the
standalone command exits unsuccessfully when validation errors are found so it
can be used in CI.

## Preview workflow

A typical AI-assisted change looks like this:

```sh
hbox update example.com index "add a pricing section"
hbox serve example.com-preview1
hbox accept example.com 1
```

The first command creates a complete preview, the second lets you inspect it,
and the third promotes it only when you are satisfied.

