//! ext2/3/4: lê apenas o mapa de blocos livres, para o carving varrer só o
//! que está livre. (Inodes apagados de ext4 têm a árvore de extents zerada,
//! então a recuperação é por assinatura.)

use super::{merge_extents, Ctx, VolumeScan};
use crate::source::read_vec;
use crate::types::Extent;

pub fn scan(ctx: &mut Ctx, base: u64, part_len: u64) -> VolumeScan {
    let mut res = VolumeScan::default();
    let Some(sb) = read_vec(ctx.src, base + 1024, 1024)
        .ok()
        .filter(|s| s.len() == 1024)
    else {
        return res;
    };
    let le32 = |o: usize| u32::from_le_bytes(sb[o..o + 4].try_into().unwrap()) as u64;
    let le16 = |o: usize| u16::from_le_bytes(sb[o..o + 2].try_into().unwrap()) as usize;
    if le16(56) != 0xEF53 {
        return res;
    }
    let bs = 1024u64 << le32(24).min(6);
    let incompat = le32(96);
    let is64 = incompat & 0x80 != 0;
    let mut blocks = le32(4);
    if is64 {
        blocks |= le32(336) << 32;
    }
    let first_data = le32(20);
    let bpg = le32(32).max(1);
    let desc_size = if is64 { le16(254).max(64) } else { 32 };
    let groups = blocks.saturating_sub(first_data).div_ceil(bpg);
    let gdt_block = first_data + 1;
    let gdt_len = groups * desc_size as u64;
    let Some(gdt) = read_vec(ctx.src, base + gdt_block * bs, gdt_len as usize)
        .ok()
        .filter(|g| g.len() as u64 == gdt_len)
    else {
        res.warnings
            .push("ext: tabela de grupos ilegível; varrendo a partição inteira.".into());
        return res;
    };
    let mut free: Vec<Extent> = vec![];
    for g in 0..groups {
        if (ctx.cancelled)() {
            break;
        }
        let d = &gdt[(g * desc_size as u64) as usize..];
        let mut bb = u32::from_le_bytes(d[0..4].try_into().unwrap()) as u64;
        if is64 && desc_size >= 64 {
            bb |= (u32::from_le_bytes(d[0x20..0x24].try_into().unwrap()) as u64) << 32;
        }
        let flags = u16::from_le_bytes(d[0x12..0x14].try_into().unwrap());
        let gstart = first_data + g * bpg;
        let gblocks = bpg.min(blocks.saturating_sub(gstart));
        if flags & 0x2 != 0 {
            // BLOCK_UNINIT: grupo sem bitmap válido; trata como livre.
            free.push(Extent::new(base + gstart * bs, gblocks * bs));
            continue;
        }
        let Some(bm) = read_vec(ctx.src, base + bb * bs, (gblocks.div_ceil(8)) as usize).ok()
        else {
            continue;
        };
        let mut run: Option<u64> = None;
        for i in 0..gblocks {
            let used = bm
                .get((i / 8) as usize)
                .map(|b| b >> (i % 8) & 1 == 1)
                .unwrap_or(true);
            match (used, run) {
                (false, None) => run = Some(i),
                (true, Some(s)) => {
                    free.push(Extent::new(base + (gstart + s) * bs, (i - s) * bs));
                    run = None;
                }
                _ => {}
            }
        }
        if let Some(s) = run {
            free.push(Extent::new(base + (gstart + s) * bs, (gblocks - s) * bs));
        }
        if g % 64 == 0 {
            (ctx.progress)(g, groups);
        }
    }
    let _ = part_len;
    res.free = Some(merge_extents(free));
    res
}
