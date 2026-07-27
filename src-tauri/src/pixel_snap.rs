use image::{ImageBuffer, Rgba, RgbaImage};
use std::cmp::Ordering;
use std::collections::{BTreeMap, HashMap, HashSet};

const MAX_PALETTE_COLORS: usize = 256;

#[derive(Debug, Clone)]
pub(crate) struct PixelSnapOptions {
    pub enabled: bool,
    pub color_count: usize,
    pub pixel_size: Option<f64>,
    pub palette: String,
    pub lock_grid: bool,
}

impl Default for PixelSnapOptions {
    fn default() -> Self {
        Self {
            enabled: false,
            color_count: 16,
            pixel_size: None,
            palette: String::new(),
            lock_grid: true,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PixelGrid {
    pub input_width: u32,
    pub input_height: u32,
    columns: Vec<usize>,
    rows: Vec<usize>,
}

#[derive(Debug, Clone)]
pub(crate) struct SnappedImage {
    pub image: RgbaImage,
    pub grid: PixelGrid,
}

#[derive(Debug, Clone)]
struct Config {
    color_count: usize,
    pixel_size_override: Option<f64>,
    palette: Option<Vec<[u8; 3]>>,
    peak_threshold_multiplier: f64,
    peak_distance_filter: usize,
    walker_search_window_ratio: f64,
    walker_min_search_window: f64,
    walker_strength_threshold: f64,
    min_cuts_per_axis: usize,
    fallback_target_segments: usize,
    max_step_ratio: f64,
}

impl Config {
    fn from_options(options: &PixelSnapOptions) -> Result<Self, String> {
        if !(2..=MAX_PALETTE_COLORS).contains(&options.color_count) {
            return Err(format!(
                "Pixel Snap color count must be between 2 and {MAX_PALETTE_COLORS}"
            ));
        }
        if let Some(pixel_size) = options.pixel_size {
            if !pixel_size.is_finite() || pixel_size < 1.0 {
                return Err("Pixel Snap pixel size must be at least 1".to_string());
            }
        }
        Ok(Self {
            color_count: options.color_count,
            pixel_size_override: options.pixel_size,
            palette: parse_palette(&options.palette)?,
            peak_threshold_multiplier: 0.2,
            peak_distance_filter: 4,
            walker_search_window_ratio: 0.35,
            walker_min_search_window: 2.0,
            walker_strength_threshold: 0.5,
            min_cuts_per_axis: 4,
            fallback_target_segments: 64,
            max_step_ratio: 1.8,
        })
    }
}

/// Applies the Sprite Fusion Pixel Snapper processing model to a complete
/// animation batch. Auto mode optionally locks every frame to the median
/// detected grid so frame dimensions cannot "breathe" during playback.
#[cfg(test)]
pub(crate) fn snap_images(
    images: &[RgbaImage],
    options: &PixelSnapOptions,
) -> Result<Vec<RgbaImage>, String> {
    if !options.enabled {
        return Ok(images.to_vec());
    }
    snap_images_with_grids(images, options)
        .map(|images| images.into_iter().map(|image| image.image).collect())
}

pub(crate) fn snap_images_with_grids(
    images: &[RgbaImage],
    options: &PixelSnapOptions,
) -> Result<Vec<SnappedImage>, String> {
    if !options.enabled {
        return Err("Pixel Snap grid output requires Pixel Snap to be enabled".to_string());
    }
    let mut config = Config::from_options(options)?;
    if images.is_empty() {
        return Ok(Vec::new());
    }
    for image in images {
        validate_dimensions(image, config.pixel_size_override)?;
    }

    if config.pixel_size_override.is_none() && options.lock_grid && images.len() > 1 {
        let mut detected = images
            .iter()
            .map(|image| detect_pixel_size(image, &config))
            .collect::<Result<Vec<_>, _>>()?;
        detected.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
        config.pixel_size_override = Some(detected[detected.len() / 2].max(1.0));
    }

    images
        .iter()
        .map(|image| snap_image(image, &config))
        .collect()
}

pub(crate) fn resample_linked_channel(
    image: &RgbaImage,
    grid: &PixelGrid,
) -> Result<RgbaImage, String> {
    if image.width() != grid.input_width || image.height() != grid.input_height {
        return Err(format!(
            "Linked channel dimensions must match the color source grid ({}x{}), got {}x{}",
            grid.input_width,
            grid.input_height,
            image.width(),
            image.height()
        ));
    }
    resample(image, &grid.columns, &grid.rows)
}

fn validate_dimensions(image: &RgbaImage, pixel_size: Option<f64>) -> Result<(), String> {
    let (width, height) = image.dimensions();
    if width < 3 || height < 3 {
        return Err(format!(
            "Pixel Snap requires images of at least 3x3 pixels; got {width}x{height}"
        ));
    }
    if let Some(pixel_size) = pixel_size {
        let max = width.min(height) as f64 / 2.0;
        if pixel_size > max {
            return Err(format!(
                "Pixel Snap pixel size {pixel_size:.1} exceeds half the smallest image dimension ({max:.1})"
            ));
        }
    }
    Ok(())
}

fn detect_pixel_size(image: &RgbaImage, config: &Config) -> Result<f64, String> {
    let analysis = quantize_image(image, config.color_count);
    let (profile_x, profile_y) = compute_profiles(&analysis)?;
    let (step_x, _) = resolve_step_sizes(
        estimate_step_size(&profile_x, config),
        estimate_step_size(&profile_y, config),
        image.width(),
        image.height(),
        config,
    );
    Ok(step_x)
}

fn snap_image(image: &RgbaImage, config: &Config) -> Result<SnappedImage, String> {
    let input_width = image.width();
    let input_height = image.height();
    let analysis = quantize_image(image, config.color_count);
    let (profile_x, profile_y) = compute_profiles(&analysis)?;
    let (step_x, step_y) = resolve_step_sizes(
        estimate_step_size(&profile_x, config),
        estimate_step_size(&profile_y, config),
        image.width(),
        image.height(),
        config,
    );
    let (columns, rows) = if config.pixel_size_override.is_some() {
        (
            snap_uniform_cuts(&profile_x, image.width() as usize, step_x, config, 2),
            snap_uniform_cuts(&profile_y, image.height() as usize, step_y, config, 2),
        )
    } else {
        let raw_columns = walk(&profile_x, step_x, image.width() as usize, config)?;
        let raw_rows = walk(&profile_y, step_y, image.height() as usize, config)?;
        stabilize_both_axes(
            &profile_x,
            &profile_y,
            raw_columns,
            raw_rows,
            image.width() as usize,
            image.height() as usize,
            config,
        )
    };
    let snapped = resample(&analysis, &columns, &rows)?;
    let image = match config.palette.as_deref() {
        Some(palette) => apply_palette(&snapped, palette),
        None => Ok(snapped),
    }?;
    Ok(SnappedImage {
        image,
        grid: PixelGrid {
            input_width,
            input_height,
            columns,
            rows,
        },
    })
}

fn quantize_image(image: &RgbaImage, color_count: usize) -> RgbaImage {
    let mut histogram = BTreeMap::<[u8; 3], usize>::new();
    for pixel in image.pixels().filter(|pixel| pixel[3] > 0) {
        *histogram.entry([pixel[0], pixel[1], pixel[2]]).or_default() += 1;
    }
    if histogram.len() <= color_count || histogram.is_empty() {
        return image.clone();
    }

    let colors = histogram
        .into_iter()
        .map(|(rgb, weight)| ([rgb[0] as f64, rgb[1] as f64, rgb[2] as f64], weight as f64))
        .collect::<Vec<_>>();
    let mut centroids = Vec::<[f64; 3]>::with_capacity(color_count);
    let first = colors
        .iter()
        .max_by(|left, right| {
            left.1
                .partial_cmp(&right.1)
                .unwrap_or(Ordering::Equal)
                .then_with(|| left.0.partial_cmp(&right.0).unwrap_or(Ordering::Equal))
        })
        .map(|entry| entry.0)
        .unwrap_or([0.0; 3]);
    centroids.push(first);

    while centroids.len() < color_count {
        let next = colors
            .iter()
            .max_by(|left, right| {
                let left_score = nearest_distance_sq(left.0, &centroids) * left.1;
                let right_score = nearest_distance_sq(right.0, &centroids) * right.1;
                left_score
                    .partial_cmp(&right_score)
                    .unwrap_or(Ordering::Equal)
                    .then_with(|| left.0.partial_cmp(&right.0).unwrap_or(Ordering::Equal))
            })
            .map(|entry| entry.0)
            .unwrap_or(first);
        if centroids.contains(&next) {
            break;
        }
        centroids.push(next);
    }

    for _ in 0..15 {
        let mut sums = vec![[0.0; 3]; centroids.len()];
        let mut weights = vec![0.0; centroids.len()];
        for (color, weight) in &colors {
            let index = nearest_centroid_index(*color, &centroids);
            for channel in 0..3 {
                sums[index][channel] += color[channel] * *weight;
            }
            weights[index] += *weight;
        }
        let mut movement: f64 = 0.0;
        for index in 0..centroids.len() {
            if weights[index] == 0.0 {
                continue;
            }
            let next = [
                sums[index][0] / weights[index],
                sums[index][1] / weights[index],
                sums[index][2] / weights[index],
            ];
            movement = movement.max(color_distance_sq(centroids[index], next));
            centroids[index] = next;
        }
        if movement < 0.01 {
            break;
        }
    }

    let mut output = RgbaImage::new(image.width(), image.height());
    let mut cache = HashMap::<[u8; 3], [u8; 3]>::new();
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] == 0 {
            output.put_pixel(x, y, *pixel);
            continue;
        }
        let key = [pixel[0], pixel[1], pixel[2]];
        let rgb = *cache.entry(key).or_insert_with(|| {
            let color = [key[0] as f64, key[1] as f64, key[2] as f64];
            let centroid = centroids[nearest_centroid_index(color, &centroids)];
            [
                centroid[0].round().clamp(0.0, 255.0) as u8,
                centroid[1].round().clamp(0.0, 255.0) as u8,
                centroid[2].round().clamp(0.0, 255.0) as u8,
            ]
        });
        output.put_pixel(x, y, Rgba([rgb[0], rgb[1], rgb[2], pixel[3]]));
    }
    output
}

fn nearest_distance_sq(color: [f64; 3], centroids: &[[f64; 3]]) -> f64 {
    centroids
        .iter()
        .map(|centroid| color_distance_sq(color, *centroid))
        .fold(f64::INFINITY, f64::min)
}

fn nearest_centroid_index(color: [f64; 3], centroids: &[[f64; 3]]) -> usize {
    centroids
        .iter()
        .enumerate()
        .min_by(|(_, left), (_, right)| {
            color_distance_sq(color, **left)
                .partial_cmp(&color_distance_sq(color, **right))
                .unwrap_or(Ordering::Equal)
        })
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn color_distance_sq(left: [f64; 3], right: [f64; 3]) -> f64 {
    (left[0] - right[0]).powi(2) + (left[1] - right[1]).powi(2) + (left[2] - right[2]).powi(2)
}

fn parse_palette(value: &str) -> Result<Option<Vec<[u8; 3]>>, String> {
    if value.trim().is_empty() {
        return Ok(None);
    }
    let mut seen = HashSet::new();
    let mut palette = Vec::new();
    for part in value.split(',') {
        let hex = part.trim().trim_start_matches('#');
        if hex.len() != 6 || !hex.chars().all(|character| character.is_ascii_hexdigit()) {
            return Err(format!(
                "Invalid Pixel Snap palette color '{}'; use comma-separated 6-digit hex colors",
                part.trim()
            ));
        }
        let color = [
            u8::from_str_radix(&hex[0..2], 16).unwrap(),
            u8::from_str_radix(&hex[2..4], 16).unwrap(),
            u8::from_str_radix(&hex[4..6], 16).unwrap(),
        ];
        if seen.insert(color) {
            palette.push(color);
        }
    }
    if palette.len() > MAX_PALETTE_COLORS {
        return Err(format!(
            "Pixel Snap palette supports at most {MAX_PALETTE_COLORS} distinct colors"
        ));
    }
    Ok(Some(palette))
}

fn apply_palette(image: &RgbaImage, palette: &[[u8; 3]]) -> Result<RgbaImage, String> {
    if palette.is_empty() {
        return Err("Pixel Snap palette must contain at least one color".to_string());
    }
    let mut cache = HashMap::<[u8; 3], [u8; 3]>::new();
    let mut output = RgbaImage::new(image.width(), image.height());
    for (x, y, pixel) in image.enumerate_pixels() {
        if pixel[3] == 0 {
            output.put_pixel(x, y, *pixel);
            continue;
        }
        let key = [pixel[0], pixel[1], pixel[2]];
        let color = *cache.entry(key).or_insert_with(|| {
            palette
                .iter()
                .min_by_key(|candidate| {
                    let red = key[0] as i32 - candidate[0] as i32;
                    let green = key[1] as i32 - candidate[1] as i32;
                    let blue = key[2] as i32 - candidate[2] as i32;
                    red * red + green * green + blue * blue
                })
                .copied()
                .unwrap_or(key)
        });
        output.put_pixel(x, y, Rgba([color[0], color[1], color[2], pixel[3]]));
    }
    Ok(output)
}

fn compute_profiles(image: &RgbaImage) -> Result<(Vec<f64>, Vec<f64>), String> {
    let (width, height) = image.dimensions();
    if width < 3 || height < 3 {
        return Err("Pixel Snap cannot analyze images smaller than 3x3".to_string());
    }
    let gray = |x, y| {
        let pixel = image.get_pixel(x, y);
        if pixel[3] == 0 {
            0.0
        } else {
            0.299 * pixel[0] as f64 + 0.587 * pixel[1] as f64 + 0.114 * pixel[2] as f64
        }
    };
    let mut columns = vec![0.0; width as usize];
    let mut rows = vec![0.0; height as usize];
    for y in 0..height {
        for x in 1..width - 1 {
            columns[x as usize] += (gray(x + 1, y) - gray(x - 1, y)).abs();
        }
    }
    for x in 0..width {
        for y in 1..height - 1 {
            rows[y as usize] += (gray(x, y + 1) - gray(x, y - 1)).abs();
        }
    }
    Ok((columns, rows))
}

fn estimate_step_size(profile: &[f64], config: &Config) -> Option<f64> {
    let maximum = profile.iter().copied().fold(0.0, f64::max);
    if maximum == 0.0 || profile.len() < 3 {
        return None;
    }
    let threshold = maximum * config.peak_threshold_multiplier;
    let peaks = (1..profile.len() - 1)
        .filter(|index| {
            profile[*index] > threshold
                && profile[*index] > profile[*index - 1]
                && profile[*index] > profile[*index + 1]
        })
        .collect::<Vec<_>>();
    let mut clean = Vec::new();
    for peak in peaks {
        if clean
            .last()
            .map(|last| peak - last > config.peak_distance_filter.saturating_sub(1))
            .unwrap_or(true)
        {
            clean.push(peak);
        }
    }
    if clean.len() < 2 {
        return None;
    }
    let mut differences = clean
        .windows(2)
        .map(|pair| (pair[1] - pair[0]) as f64)
        .collect::<Vec<_>>();
    differences.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    Some(differences[differences.len() / 2])
}

fn resolve_step_sizes(
    step_x: Option<f64>,
    step_y: Option<f64>,
    width: u32,
    height: u32,
    config: &Config,
) -> (f64, f64) {
    if let Some(pixel_size) = config.pixel_size_override {
        return (pixel_size, pixel_size);
    }
    match (step_x, step_y) {
        (Some(x), Some(y)) => {
            let ratio = x.max(y) / x.min(y).max(f64::EPSILON);
            let step = if ratio > config.max_step_ratio {
                x.min(y)
            } else {
                (x + y) / 2.0
            };
            (step, step)
        }
        (Some(step), None) | (None, Some(step)) => (step, step),
        (None, None) => {
            let fallback =
                (width.min(height) as f64 / config.fallback_target_segments as f64).max(1.0);
            (fallback, fallback)
        }
    }
}

fn walk(
    profile: &[f64],
    step_size: f64,
    limit: usize,
    config: &Config,
) -> Result<Vec<usize>, String> {
    if profile.is_empty() {
        return Err("Pixel Snap cannot walk an empty edge profile".to_string());
    }
    let mut cuts = vec![0];
    let mut current = 0.0;
    let window =
        (step_size * config.walker_search_window_ratio).max(config.walker_min_search_window);
    let mean = profile.iter().sum::<f64>() / profile.len() as f64;
    while current < limit as f64 {
        let target = current + step_size;
        if target >= limit as f64 {
            cuts.push(limit);
            break;
        }
        let start = ((target - window).max(current + 1.0)) as usize;
        let end = ((target + window) as usize).min(limit);
        if end <= start {
            current = target;
            continue;
        }
        let (best_index, best_value) = profile
            .iter()
            .enumerate()
            .take(end)
            .skip(start)
            .max_by(|left, right| left.1.partial_cmp(right.1).unwrap_or(Ordering::Equal))
            .map(|(index, value)| (index, *value))
            .unwrap_or((start, 0.0));
        if best_value > mean * config.walker_strength_threshold {
            cuts.push(best_index);
            current = best_index as f64;
        } else {
            cuts.push(target as usize);
            current = target;
        }
    }
    Ok(cuts)
}

fn stabilize_both_axes(
    profile_x: &[f64],
    profile_y: &[f64],
    columns: Vec<usize>,
    rows: Vec<usize>,
    width: usize,
    height: usize,
    config: &Config,
) -> (Vec<usize>, Vec<usize>) {
    let columns = stabilize_cuts(profile_x, columns, width, &rows, height, config);
    let rows = stabilize_cuts(profile_y, rows, height, &columns, width, config);
    let column_step = width as f64 / columns.len().saturating_sub(1).max(1) as f64;
    let row_step = height as f64 / rows.len().saturating_sub(1).max(1) as f64;
    if column_step.max(row_step) / column_step.min(row_step).max(f64::EPSILON)
        <= config.max_step_ratio
    {
        return (columns, rows);
    }
    let target = column_step.min(row_step);
    (
        if column_step > target * 1.2 {
            snap_uniform_cuts(profile_x, width, target, config, config.min_cuts_per_axis)
        } else {
            columns
        },
        if row_step > target * 1.2 {
            snap_uniform_cuts(profile_y, height, target, config, config.min_cuts_per_axis)
        } else {
            rows
        },
    )
}

fn stabilize_cuts(
    profile: &[f64],
    cuts: Vec<usize>,
    limit: usize,
    sibling_cuts: &[usize],
    sibling_limit: usize,
    config: &Config,
) -> Vec<usize> {
    let cuts = sanitize_cuts(cuts, limit);
    let minimum = if config.pixel_size_override.is_some() {
        2
    } else {
        config.min_cuts_per_axis.max(2)
    }
    .min(limit.saturating_add(1));
    let cells = cuts.len().saturating_sub(1);
    let sibling_cells = sibling_cuts.len().saturating_sub(1);
    let sibling_has_grid = sibling_limit > 0 && sibling_cells >= minimum.saturating_sub(1);
    let skewed = sibling_has_grid && cells > 0 && {
        let ratio =
            (limit as f64 / cells as f64) / (sibling_limit as f64 / sibling_cells.max(1) as f64);
        ratio > config.max_step_ratio || ratio < 1.0 / config.max_step_ratio
    };
    if cuts.len() >= minimum && !skewed {
        return cuts;
    }
    let target = if sibling_has_grid {
        sibling_limit as f64 / sibling_cells as f64
    } else {
        limit as f64 / config.fallback_target_segments.max(1) as f64
    }
    .max(1.0);
    snap_uniform_cuts(profile, limit, target, config, minimum)
}

fn sanitize_cuts(mut cuts: Vec<usize>, limit: usize) -> Vec<usize> {
    cuts.push(0);
    cuts.push(limit);
    for cut in &mut cuts {
        *cut = (*cut).min(limit);
    }
    cuts.sort_unstable();
    cuts.dedup();
    cuts
}

fn snap_uniform_cuts(
    profile: &[f64],
    limit: usize,
    target_step: f64,
    config: &Config,
    minimum: usize,
) -> Vec<usize> {
    if limit <= 1 {
        return vec![0, limit];
    }
    let cells = ((limit as f64 / target_step).round() as usize)
        .max(minimum.saturating_sub(1))
        .clamp(1, limit);
    let width = limit as f64 / cells as f64;
    let window = (width * config.walker_search_window_ratio).max(config.walker_min_search_window);
    let mean = profile.iter().sum::<f64>() / profile.len().max(1) as f64;
    let mut cuts = vec![0];
    for index in 1..cells {
        let target = width * index as f64;
        let previous = *cuts.last().unwrap();
        if previous + 1 >= limit {
            break;
        }
        let start = ((target - window).floor().max(previous as f64 + 1.0)) as usize;
        let end = ((target + window).ceil() as usize).min(limit - 1);
        let (candidate, strength) = (start..=end)
            .map(|position| (position, profile.get(position).copied().unwrap_or(0.0)))
            .max_by(|left, right| left.1.partial_cmp(&right.1).unwrap_or(Ordering::Equal))
            .unwrap_or((target.round() as usize, 0.0));
        let fallback = (target.round() as usize).clamp(previous + 1, limit - 1);
        cuts.push(if strength >= mean * config.walker_strength_threshold {
            candidate
        } else {
            fallback
        });
    }
    sanitize_cuts(cuts, limit)
}

fn resample(image: &RgbaImage, columns: &[usize], rows: &[usize]) -> Result<RgbaImage, String> {
    if columns.len() < 2 || rows.len() < 2 {
        return Err("Pixel Snap found too few grid cuts to resample the image".to_string());
    }
    let mut output: RgbaImage =
        ImageBuffer::new((columns.len() - 1) as u32, (rows.len() - 1) as u32);
    for (output_y, vertical) in rows.windows(2).enumerate() {
        for (output_x, horizontal) in columns.windows(2).enumerate() {
            let mut counts = HashMap::<[u8; 4], usize>::new();
            for y in vertical[0]..vertical[1] {
                for x in horizontal[0]..horizontal[1] {
                    if x < image.width() as usize && y < image.height() as usize {
                        *counts
                            .entry(image.get_pixel(x as u32, y as u32).0)
                            .or_default() += 1;
                    }
                }
            }
            let color = counts
                .into_iter()
                .max_by(|left, right| left.1.cmp(&right.1).then_with(|| right.0.cmp(&left.0)))
                .map(|entry| entry.0)
                .unwrap_or([0, 0, 0, 0]);
            output.put_pixel(output_x as u32, output_y as u32, Rgba(color));
        }
    }
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_grid_reduces_blocks_to_logical_pixels() {
        let mut image = RgbaImage::from_pixel(8, 8, Rgba([0, 0, 0, 0]));
        for y in 0..4 {
            for x in 0..4 {
                image.put_pixel(x, y, Rgba([255, 0, 0, 255]));
            }
        }
        let output = snap_images(
            &[image],
            &PixelSnapOptions {
                enabled: true,
                color_count: 4,
                pixel_size: Some(4.0),
                palette: String::new(),
                lock_grid: true,
            },
        )
        .unwrap();
        assert_eq!(output[0].dimensions(), (2, 2));
        assert_eq!(output[0].get_pixel(0, 0).0, [255, 0, 0, 255]);
    }

    #[test]
    fn custom_palette_is_applied_after_snapping() {
        let image = RgbaImage::from_pixel(8, 8, Rgba([240, 20, 30, 255]));
        let output = snap_images(
            &[image],
            &PixelSnapOptions {
                enabled: true,
                color_count: 4,
                pixel_size: Some(2.0),
                palette: "000000,ffffff".to_string(),
                lock_grid: true,
            },
        )
        .unwrap();
        assert_eq!(output[0].get_pixel(0, 0).0, [0, 0, 0, 255]);
    }
}
