//! Pure-Rust ISO/IEC 18004 QR Code encoder (Versions 1-10, Byte mode, ECC-M) + Cairo renderer.

use crate::ffi::cairo_t;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrMatrix {
    pub version: u8,
    pub size: usize,
    pub modules: Vec<bool>,
}

impl QrMatrix {
    pub fn get(&self, x: usize, y: usize) -> bool {
        self.modules[y * self.size + x]
    }
}

struct VersionSpec {
    version: u8,
    ec_per_block: usize,
    g1_blocks: usize,
    g1_data: usize,
    g2_blocks: usize,
    g2_data: usize,
    align: &'static [usize],
}

const SPECS: &[VersionSpec] = &[
    VersionSpec { version: 1, ec_per_block: 10, g1_blocks: 1, g1_data: 16, g2_blocks: 0, g2_data: 0, align: &[] },
    VersionSpec { version: 2, ec_per_block: 16, g1_blocks: 1, g1_data: 28, g2_blocks: 0, g2_data: 0, align: &[6, 18] },
    VersionSpec { version: 3, ec_per_block: 26, g1_blocks: 1, g1_data: 44, g2_blocks: 0, g2_data: 0, align: &[6, 22] },
    VersionSpec { version: 4, ec_per_block: 18, g1_blocks: 2, g1_data: 32, g2_blocks: 0, g2_data: 0, align: &[6, 26] },
    VersionSpec { version: 5, ec_per_block: 24, g1_blocks: 2, g1_data: 43, g2_blocks: 0, g2_data: 0, align: &[6, 30] },
    VersionSpec { version: 6, ec_per_block: 16, g1_blocks: 4, g1_data: 27, g2_blocks: 0, g2_data: 0, align: &[6, 34] },
    VersionSpec { version: 7, ec_per_block: 18, g1_blocks: 4, g1_data: 31, g2_blocks: 0, g2_data: 0, align: &[6, 22, 38] },
    VersionSpec { version: 8, ec_per_block: 22, g1_blocks: 2, g1_data: 38, g2_blocks: 2, g2_data: 39, align: &[6, 24, 42] },
    VersionSpec { version: 9, ec_per_block: 22, g1_blocks: 3, g1_data: 36, g2_blocks: 2, g2_data: 37, align: &[6, 26, 46] },
    VersionSpec { version: 10, ec_per_block: 26, g1_blocks: 4, g1_data: 43, g2_blocks: 1, g2_data: 44, align: &[6, 28, 50] },
];

impl VersionSpec {
    fn total_data_bytes(&self) -> usize {
        self.g1_blocks * self.g1_data + self.g2_blocks * self.g2_data
    }

    fn max_payload_bytes(&self) -> usize {
        let overhead_bits = if self.version < 10 { 12 } else { 20 };
        (self.total_data_bytes() * 8 - overhead_bits) / 8
    }
}

fn gf_mul(mut a: u8, mut b: u8) -> u8 {
    let mut res = 0u8;
    for _ in 0..8 {
        if (b & 1) != 0 { res ^= a; }
        let hi = (a & 0x80) != 0;
        a <<= 1;
        if hi { a ^= 0x1d; }
        b >>= 1;
    }
    res
}

fn rs_generator(degree: usize) -> Vec<u8> {
    let mut poly = vec![0u8; degree];
    poly[degree - 1] = 1;
    let mut root = 1u8;
    for _ in 0..degree {
        rs_mul_step(&mut poly, root);
        root = gf_mul(root, 0x02);
    }
    poly
}

fn rs_mul_step(poly: &mut [u8], root: u8) {
    for j in 0..poly.len() {
        poly[j] = gf_mul(poly[j], root);
        if j + 1 < poly.len() {
            poly[j] ^= poly[j + 1];
        }
    }
}

fn rs_remainder(data: &[u8], gen: &[u8]) -> Vec<u8> {
    let mut rem = vec![0u8; gen.len()];
    for &byte in data {
        let factor = byte ^ rem[0];
        rem.rotate_left(1);
        *rem.last_mut().unwrap() = 0;
        for (slot, &g) in rem.iter_mut().zip(gen.iter()) {
            *slot ^= gf_mul(g, factor);
        }
    }
    rem
}

fn push_bits(bits: &mut Vec<bool>, val: u32, count: usize) {
    for shift in (0..count).rev() {
        bits.push(((val >> shift) & 1) != 0);
    }
}

fn build_bitstream(payload: &[u8], spec: &VersionSpec) -> Vec<bool> {
    let cap_bits = spec.total_data_bytes() * 8;
    let mut bits = Vec::with_capacity(cap_bits);
    push_bits(&mut bits, 0b0100, 4);
    push_bits(&mut bits, payload.len() as u32, if spec.version < 10 { 8 } else { 16 });
    for &b in payload {
        push_bits(&mut bits, u32::from(b), 8);
    }
    let term = (cap_bits - bits.len()).min(4);
    push_bits(&mut bits, 0, term);
    while bits.len() % 8 != 0 {
        bits.push(false);
    }
    bits
}

fn encode_data_codewords(payload: &[u8], spec: &VersionSpec) -> Vec<u8> {
    let bits = build_bitstream(payload, spec);
    let mut bytes: Vec<u8> = bits
        .chunks_exact(8)
        .map(|ch| ch.iter().fold(0u8, |acc, &bit| (acc << 1) | u8::from(bit)))
        .collect();
    let pad = [0xec, 0x11];
    let mut idx = 0;
    while bytes.len() < spec.total_data_bytes() {
        bytes.push(pad[idx % 2]);
        idx += 1;
    }
    bytes
}

fn collect_rs_blocks<'a>(data: &'a [u8], spec: &VersionSpec) -> (Vec<&'a [u8]>, Vec<Vec<u8>>) {
    let gen = rs_generator(spec.ec_per_block);
    let (mut d_blocks, mut e_blocks, mut offset) = (Vec::new(), Vec::new(), 0);
    for (count, len) in [(spec.g1_blocks, spec.g1_data), (spec.g2_blocks, spec.g2_data)] {
        for _ in 0..count {
            let block = &data[offset..offset + len];
            offset += len;
            e_blocks.push(rs_remainder(block, &gen));
            d_blocks.push(block);
        }
    }
    (d_blocks, e_blocks)
}

fn split_and_interleave(data: &[u8], spec: &VersionSpec) -> Vec<u8> {
    let (data_blocks, ec_blocks) = collect_rs_blocks(data, spec);
    let mut out = Vec::new();
    for i in 0..spec.g1_data.max(spec.g2_data) {
        for block in &data_blocks {
            if i < block.len() { out.push(block[i]); }
        }
    }
    for i in 0..spec.ec_per_block {
        for ec in &ec_blocks {
            out.push(ec[i]);
        }
    }
    out
}

struct Grid {
    size: usize,
    modules: Vec<bool>,
    is_func: Vec<bool>,
}

impl Grid {
    fn new(size: usize) -> Self {
        Self { size, modules: vec![false; size * size], is_func: vec![false; size * size] }
    }

    fn set_func(&mut self, x: usize, y: usize, dark: bool) {
        let idx = y * self.size + x;
        self.modules[idx] = dark;
        self.is_func[idx] = true;
    }

    fn place_finder(&mut self, cx: isize, cy: isize) {
        let limit = self.size as isize;
        for dy in -4..=4 {
            for dx in -4..=4 {
                let (x, y) = (cx + dx, cy + dy);
                if x >= 0 && y >= 0 && x < limit && y < limit {
                    let dist = dx.abs().max(dy.abs());
                    self.set_func(x as usize, y as usize, dist != 2 && dist != 4);
                }
            }
        }
    }

    fn place_alignment(&mut self, cx: usize, cy: usize) {
        for dy in -2..=2isize {
            for dx in -2..=2isize {
                let x = (cx as isize + dx) as usize;
                let y = (cy as isize + dy) as usize;
                self.set_func(x, y, dx.abs().max(dy.abs()) != 1);
            }
        }
    }

    fn place_alignments(&mut self, coords: &[usize]) {
        let s = self.size;
        for &cy in coords {
            for &cx in coords {
                if !self.is_func[cy * s + cx] {
                    self.place_alignment(cx, cy);
                }
            }
        }
    }

    fn place_function_patterns(&mut self, spec: &VersionSpec) {
        let s = self.size;
        for i in 0..s {
            self.set_func(6, i, i % 2 == 0);
            self.set_func(i, 6, i % 2 == 0);
        }
        self.place_finder(3, 3);
        self.place_finder(s as isize - 4, 3);
        self.place_finder(3, s as isize - 4);
        self.place_alignments(spec.align);
        self.reserve_format_bits();
        if spec.version >= 7 {
            self.place_version_bits(spec.version);
        }
    }

    fn reserve_format_bits(&mut self) {
        let s = self.size;
        for i in 0..=8 {
            if i != 6 {
                self.set_func(i, 8, false);
                self.set_func(8, i, false);
            }
        }
        for i in 0..8 { self.set_func(s - 1 - i, 8, false); }
        for i in 0..7 { self.set_func(8, s - 1 - i, false); }
        self.set_func(8, s - 8, true);
    }

    fn place_version_bits(&mut self, version: u8) {
        let (bits, s) = (version_info_bits(version), self.size);
        for b in 0..18 {
            let dark = ((bits >> b) & 1) != 0;
            let (a, c) = (s - 11 + (b % 3), b / 3);
            self.set_func(c, a, dark);
            self.set_func(a, c, dark);
        }
    }

    fn place_format_bits(&mut self, mask: u8) {
        let (bits, s) = (format_info_bits(mask), self.size);
        let bit = |pos: usize| ((bits >> pos) & 1) != 0;
        for i in 0..6 {
            self.modules[8 * s + i] = bit(14 - i);
            self.modules[(5 - i) * s + 8] = bit(5 - i);
        }
        self.modules[8 * s + 7] = bit(8);
        self.modules[8 * s + 8] = bit(7);
        self.modules[7 * s + 8] = bit(6);
        for i in 0..7 { self.modules[(s - 1 - i) * s + 8] = bit(14 - i); }
        for i in 0..8 { self.modules[8 * s + (s - 8 + i)] = bit(7 - i); }
    }

    fn write_data_cell(&mut self, x: usize, y: usize, codewords: &[u8], bit_idx: &mut usize) {
        let idx = y * self.size + x;
        if self.is_func[idx] {
            return;
        }
        let b = *bit_idx;
        let dark = b < codewords.len() * 8 && ((codewords[b / 8] >> (7 - (b % 8))) & 1) != 0;
        self.modules[idx] = dark;
        *bit_idx += 1;
    }

    fn place_codewords(&mut self, codewords: &[u8]) {
        let (s, mut bit_idx, mut right, mut upward) = (self.size, 0usize, self.size as isize - 1, true);
        while right >= 1 {
            if right == 6 { right -= 1; }
            for step in 0..s {
                let y = if upward { s - 1 - step } else { step };
                self.write_data_cell(right as usize, y, codewords, &mut bit_idx);
                self.write_data_cell((right - 1) as usize, y, codewords, &mut bit_idx);
            }
            upward = !upward;
            right -= 2;
        }
    }
}

fn format_info_bits(mask: u8) -> u32 {
    let data = u32::from(mask & 7);
    let mut rem = data << 10;
    for i in (0..5).rev() {
        if (rem & (1 << (i + 10))) != 0 { rem ^= 0x537 << i; }
    }
    ((data << 10) | rem) ^ 0x5412
}

fn version_info_bits(version: u8) -> u32 {
    let data = u32::from(version);
    let mut rem = data << 12;
    for i in (0..6).rev() {
        if (rem & (1 << (i + 12))) != 0 { rem ^= 0x1f25 << i; }
    }
    (data << 12) | rem
}

fn mask_bit(mask: u8, x: usize, y: usize) -> bool {
    match mask {
        0 => (x + y) % 2 == 0,
        1 => y % 2 == 0,
        2 => x % 3 == 0,
        3 => (x + y) % 3 == 0,
        4 => ((y / 2) + (x / 3)) % 2 == 0,
        5 => ((x * y) % 2) + ((x * y) % 3) == 0,
        6 => (((x * y) % 2) + ((x * y) % 3)) % 2 == 0,
        _ => (((x + y) % 2) + ((x * y) % 3)) % 2 == 0,
    }
}

fn apply_mask(base: &Grid, mask: u8) -> Vec<bool> {
    let mut out = base.modules.clone();
    let s = base.size;
    for y in 0..s {
        for x in 0..s {
            let idx = y * s + x;
            if !base.is_func[idx] && mask_bit(mask, x, y) { out[idx] = !out[idx]; }
        }
    }
    let mut tmp = Grid { size: s, modules: out, is_func: Vec::new() };
    tmp.place_format_bits(mask);
    tmp.modules
}

fn line_run_penalty(iter: impl Iterator<Item = bool>) -> usize {
    let (mut score, mut prev, mut run) = (0usize, None, 0usize);
    for cur in iter {
        if prev == Some(cur) { run += 1; }
        else { if run >= 5 { score += run - 2; } prev = Some(cur); run = 1; }
    }
    if run >= 5 { score += run - 2; }
    score
}

fn penalty_runs(modules: &[bool], size: usize) -> usize {
    let mut score = 0;
    for i in 0..size {
        score += line_run_penalty((0..size).map(|x| modules[i * size + x]));
        score += line_run_penalty((0..size).map(|y| modules[y * size + i]));
    }
    score
}

fn is_2x2_block(modules: &[bool], size: usize, x: usize, y: usize) -> bool {
    let c = modules[y * size + x];
    c == modules[y * size + x + 1] && c == modules[(y + 1) * size + x] && c == modules[(y + 1) * size + x + 1]
}

fn penalty_blocks_and_balance(modules: &[bool], size: usize) -> usize {
    let mut score = 0usize;
    for y in 0..(size - 1) {
        for x in 0..(size - 1) {
            if is_2x2_block(modules, size, x, y) { score += 3; }
        }
    }
    let pct = (modules.iter().filter(|&&b| b).count() * 100) / (size * size);
    score + (pct.abs_diff(50) / 5) * 10
}

fn pick_best_mask(grid: &Grid) -> Vec<bool> {
    (0..8u8)
        .map(|m| {
            let cand = apply_mask(grid, m);
            let score = penalty_runs(&cand, grid.size) + penalty_blocks_and_balance(&cand, grid.size);
            (score, cand)
        })
        .min_by_key(|(s, _)| *s)
        .map(|(_, m)| m)
        .unwrap_or_default()
}

pub fn encode_qr(payload: &[u8]) -> Result<QrMatrix, String> {
    let spec = SPECS
        .iter()
        .find(|s| payload.len() <= s.max_payload_bytes())
        .ok_or_else(|| format!("QR payload too large ({} bytes, max 213)", payload.len()))?;
    let size = usize::from(spec.version) * 4 + 17;
    let codewords = split_and_interleave(&encode_data_codewords(payload, spec), spec);
    let mut grid = Grid::new(size);
    grid.place_function_patterns(spec);
    grid.place_codewords(&codewords);
    Ok(QrMatrix { version: spec.version, size, modules: pick_best_mask(&grid) })
}

#[link(name = "cairo")]
extern "C" {
    fn cairo_set_source_rgb(cr: *mut cairo_t, r: f64, g: f64, b: f64);
    fn cairo_rectangle(cr: *mut cairo_t, x: f64, y: f64, w: f64, h: f64);
    fn cairo_fill(cr: *mut cairo_t);
}

fn qr_layout(size: usize, width: f64, height: f64) -> (f64, f64, f64, f64) {
    let total = (size + 8) as f64;
    let cell = (width.min(height) / total).floor().max(1.0);
    let board = cell * total;
    (((width - board) / 2.0).floor(), ((height - board) / 2.0).floor(), cell, board)
}

pub fn draw_qr_matrix(cr: *mut cairo_t, qr: &QrMatrix, width: f64, height: f64) {
    if cr.is_null() || width <= 0.0 || height <= 0.0 {
        return;
    }
    let (ox, oy, cell, board) = qr_layout(qr.size, width, height);
    unsafe {
        cairo_set_source_rgb(cr, 1.0, 1.0, 1.0);
        cairo_rectangle(cr, ox, oy, board, board);
        cairo_fill(cr);
        cairo_set_source_rgb(cr, 0.035, 0.035, 0.043);
        for y in 0..qr.size {
            for x in 0..qr.size {
                if qr.get(x, y) {
                    cairo_rectangle(cr, ox + (x + 4) as f64 * cell, oy + (y + 4) as f64 * cell, cell, cell);
                }
            }
        }
        cairo_fill(cr);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn format_and_version_bch_match_iso_18004() {
        assert_eq!(format_info_bits(0), 0x5412);
        assert_eq!(version_info_bits(7), 0x07c94);
    }

    #[test]
    fn encodes_pairing_urls_with_valid_finder_and_dark_module() {
        let url = b"http://192.168.1.42:7474/?pair=0123456789abcdef0123456789abcdef&host=pavel-laptop&v=1";
        let qr = encode_qr(url).unwrap();
        assert_eq!((qr.version, qr.size), (6, 41));
        assert!(qr.get(0, 0) && qr.get(6, 0) && qr.get(3, 3) && !qr.get(1, 1));
        assert!(qr.get(8, qr.size - 8));
    }

    #[test]
    fn reed_solomon_remainder_has_zero_syndrome() {
        let gen = rs_generator(10);
        let mut full = b"Hello, AkhTerm!!".to_vec();
        full.extend_from_slice(&rs_remainder(&full, &gen));
        assert!(rs_remainder(&full, &gen).iter().all(|&b| b == 0));
    }

    #[test]
    fn encodes_all_versions_1_through_10_and_rejects_oversize() {
        for len in [10, 24, 40, 60, 80, 100, 120, 150, 178, 210] {
            let qr = encode_qr(&vec![b'a'; len]).unwrap();
            assert_eq!(qr.modules.len(), qr.size * qr.size);
        }
        assert!(encode_qr(&vec![b'x'; 220]).is_err());
    }
}
