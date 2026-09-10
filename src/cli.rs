use std::{
    num::{NonZeroUsize,NonZeroU32},
    path::PathBuf,
};

use clap::{Args, Parser, Subcommand};

#[derive(Debug, Parser)]
#[command(name = "hbox")]
#[command(version)]
#[command(about = "A tiny static site builder optimized for AI-generated source files.")]
#[command(arg_required_else_help = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize a new Hbox site
    Init(InitArgs),

    /// Build a site into dist/<site-name>
    Build(BuildArgs),

    #[command(name = "import")]
    /// Spawn a site or a single page from an image
    ImportDesign(ImportDesignArgs),

    /// Update a page via LLM
    Update(UpdateDesignArgs),

    /// Accept a preview, keeping only 1 version
    #[command(name = "accept")]
    AcceptPreview(AcceptPreviewArgs),

    /// Optimize site, convert images to webp, add src-sets, etc
    Optimize(OptimizeArgs),

    /// Validate all images, links and fragments
    Validate(ValidateArgs),

    /// Serve site on localhost with hot-reloads/rebuilds
    Preview(PreviewArgs),

}

#[derive(Debug, Args)]
pub struct InitArgs {
    /// Path to initialize
    pub site_name: PathBuf,

    /// Overwrite existing starter files
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Args)]
pub struct ImportDesignArgs {
    /// Site to add page to
    pub site_name: PathBuf,

    /// Path to inspirational screenshot
    pub screenshot_path: PathBuf,

    /// Name of page (slug)
    pub slug: String,

    #[arg(long = "threads", short = 't', default_value = "2")]
    /// Concurrent calls to LLM backend (beware of ratelimits)
    pub threads: NonZeroUsize,
}

#[derive(Debug, Args)]
pub struct AcceptPreviewArgs {
   #[arg(index=1)]
    /// Site which has previews
    pub site_name: PathBuf,

    /// Preview number to accept
    #[arg(index=2)]
    pub preview_num: NonZeroU32,
}

#[derive(Debug, Args)]
pub struct UpdateDesignArgs {
    /// Site to add page to
    pub site_name: PathBuf,
    /// Name of page (slug)
    pub slug: String,
    /// Query sent to the LLM (ie. a prompt)
    pub prompt: String,

    #[arg(long = "threads", short = 't', default_value = "2")]
    /// Concurrent calls to LLM backend (beware of ratelimits)
    pub threads: NonZeroUsize,
}

#[derive(Debug, Args)]
pub struct BuildArgs {
    /// Name of the site, e.g. lbjgruppen.com
    pub site: PathBuf,
}

#[derive(Debug, Args)]
pub struct OptimizeArgs {
    /// Name of the site, e.g. lbjgruppen.com
    pub site_name: PathBuf,
}

#[derive(Debug, Args)]
pub struct PreviewArgs {
    /// Name of the site, e.g. lbjgruppen.com
    pub site: PathBuf,

    /// Preview number to serve
    pub preview_index: Option<NonZeroU32>,

    /// Port to serve on
    #[arg(long, default_value_t = 8080)]
    pub port: u16,
}

#[derive(Debug, Args)]
pub struct ValidateArgs {
    /// Name of the site, e.g. lbjgruppen.com
    pub site: PathBuf,

    /// Verify external HTTP links?
    #[arg(long)]
    pub check_external_links: bool,
}
