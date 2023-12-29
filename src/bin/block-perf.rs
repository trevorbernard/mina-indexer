use clap::Parser;
use glob::glob;
use mina_indexer::{display_duration, mina_blocks::v1::precomputed_block::parse_file};
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Startup blocks directory path
    #[arg(short, long, default_value = concat!(env!("HOME"), ".mina-indexer/blocks/mainnet"))]
    blocks_dir: PathBuf,
}

fn main() -> anyhow::Result<()> {
    let args = Args::parse();
    let blocks_dir = args.blocks_dir;

    let paths: Vec<PathBuf> = find_and_sort_json_blocks(&blocks_dir)?;
    // let mut paths: Vec<PathBuf> = glob(&format!("{}/*.json", blocks_dir.display()))?
    //     .filter_map(|x| x.ok())
    //     .collect();

    let now = Instant::now();
    let num = paths.len();
    println!("{} precomputed blocks to be deserialized...", num);
    let mut block_count = 0;
    for path in paths {
        //      println!("Filename: {:?}", path.display());
        block_count += 1;
        let _ = parse_file(path)?;
        if block_count % 5000 == 0 {
            let display_elapsed: String = display_duration(now.elapsed());
            println!("\n~~~ General ~~~");
            println!("Blocks:  {block_count}");
            println!("Total:   {display_elapsed}");

            let blocks_per_sec = block_count as f64 / now.elapsed().as_secs_f64();
            println!("\n~~~ Block stats ~~~");
            println!("Per sec: {blocks_per_sec:?} blocks");
            println!("Per hr:  {:?} blocks", blocks_per_sec * 3600.);
        }
        //println!("{}", block_count);
    }

    let display_elapsed: String = display_duration(now.elapsed());
    println!("\n~~~ General ~~~");
    println!("Blocks:  {block_count}");
    println!("Total:   {display_elapsed}");

    let blocks_per_sec = block_count as f64 / now.elapsed().as_secs_f64();
    println!("\n~~~ Block stats ~~~");
    println!("Per sec: {blocks_per_sec:?} blocks");
    println!("Per hr:  {:?} blocks", blocks_per_sec * 3600.);
    Ok(())
}

fn find_and_sort_json_blocks(blocks_dir: &Path) -> anyhow::Result<Vec<PathBuf>> {
    let mut paths: Vec<(u64, PathBuf)> = glob(&format!("{}/*.json", blocks_dir.display()))?
        .filter_map(|x| x.ok())
        .filter_map(|path| {
            if let Some(file_name) = path.file_name().and_then(|s| s.to_str()) {
                let parts: Vec<&str> = file_name.split('-').collect();
                if parts.len() == 3 && parts[2].ends_with(".json") {
                    parts[1]
                        .parse::<u64>()
                        .ok()
                        .map(|block_height| (block_height, path))
                } else {
                    None
                }
            } else {
                None
            }
        })
        .collect();
    paths.sort_by_key(|k| k.0);
    Ok(paths.into_iter().map(|(_, path)| path).collect())
}
