# Blogging

Hbox renders Markdown files from `sites/<site>/blogposts/` through MiniJinja
templates. Each published post is written to:

```text
/blog/<language>/<slug>/index.html
```

## Creating a post

Create a `.md` file with YAML front matter followed by Markdown:

````markdown
---
title: "Functional Quadtrees"
slug: "functional-quadtrees"
language: "en"
description: "Using immutable quadtrees for a flocking simulation."
date: "2026-09-06"
template: "templates/blogpost.html"
image: "/images/blog/functional-quadtrees.png"
draft: false
code_theme: "ocean-dark"
externals:
  - "/scripts/quadtree.js"
---

Quadtree subdivision is a useful way to reduce the number of comparisons.

```clojure
(defn insert [tree point]
  (update tree :points conj point))
```

<div class="quadtree"></div>
````

Markdown may contain raw HTML when a script or custom component needs a mount
point, as in the `quadtree` element above.

## Front matter

| Field | Required | Behavior |
| --- | --- | --- |
| `title` | Yes | Display title for the post. |
| `slug` | No | URL segment. Defaults to the Markdown filename without `.md`. |
| `language` | No | URL language segment. Defaults to `site.default_language`. |
| `description` | No | Summary available to the post template and to `posts`. |
| `date` | No | Publication date in `YYYY-MM-DD` form. Dated posts are listed newest first. |
| `template` | No | MiniJinja template used to wrap the rendered Markdown. |
| `image` | No | Featured-image URL exposed as `featured_image`. |
| `draft` | No | When `true`, the post is excluded from the build and from `posts`. |
| `code_theme` | No | Overrides the site's syntax-highlighting theme for this post. |
| `externals` | No | Ordered list of script URLs inserted before the closing `</body>` tag. |

An undated published post is listed after dated posts. Two posts may not resolve
to the same output URL.

## Blog template

The default template lives at `templates/blogpost.html`. It must be a complete
HTML document:

```jinja
<!doctype html>
<html lang="{{ language }}">
  <head>
    <meta charset="utf-8">
    <meta name="description" content="{{ description }}">
    <title>{{ title }}</title>
  </head>
  <body>
    {% include "partials/header.html" %}
    <main>
      <article>
        <h1>{{ title }}</h1>
        {{ content }}
      </article>
    </main>
    {% include "partials/footer.html" %}
  </body>
</html>
```

`content` is already rendered HTML. Hbox also injects its managed stylesheets
and any scripts declared by `externals` during the build.

## Listing posts

Normal pages and blog templates receive a `posts` collection:

```jinja
{% for post in posts %}
  <article>
    <h2><a href="{{ post.url }}">{{ post.title }}</a></h2>
    {% if post.date %}<time>{{ post.date }}</time>{% endif %}
    <p>{{ post.description }}</p>
  </article>
{% endfor %}
```

Drafts never appear in this collection.

## Syntax highlighting

Use an ordinary fenced code block with a language identifier:

````markdown
```rust
fn main() {
    println!("hello");
}
```
````

The post's `code_theme` takes precedence over `site.code_theme`. See
[Configuration](Configuration.md) for the available theme names.

## Markdown inside an HTML page

Blog posts are Markdown files, but a normal `pages/*.html` template can render
a smaller Markdown region with the `markdown` filter:

```jinja
<section class="prose">
{% filter markdown %}
## Write this section in Markdown

- Simple source
- Normal HTML output
{% endfilter %}
</section>
```
