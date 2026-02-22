//! Laser blaster screen — green laser beams crossing the screen.
//!
//! Features:
//! - Laser beams firing across the screen from all edges
//! - Bright "KADI" text rendered in front of lasers
//! - LEDs flash green in bursts synced to laser fire
//!
//! Dual-core: Core 0 renders, Core 1 blits via SPI/DMA.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU8, Ordering};

use defmt::info;
#[allow(clippy::wildcard_imports)]
use disobey2026badge::*;
use embassy_executor::Spawner;
use embassy_time::{Duration, Timer};
use embedded_graphics::{
    pixelcolor::Rgb565,
    prelude::*,
    primitives::Rectangle,
};
use esp_backtrace as _;
use esp_hal::timer::timg::TimerGroup;
use esp_println as _;
use palette::Srgb;

extern crate alloc;

esp_bootloader_esp_idf::esp_app_desc!();

const W: i32 = 320;
const H: i32 = 170;
const PIXELS: usize = (W * H) as usize;

// ── Framebuffer ─────────────────────────────────────────────────────────────

struct Fb {
    buf: &'static mut [Rgb565; PIXELS],
}

impl DrawTarget for Fb {
    type Color = Rgb565;
    type Error = core::convert::Infallible;
    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where I: IntoIterator<Item = Pixel<Self::Color>> {
        for Pixel(Point { x, y }, color) in pixels {
            if x >= 0 && x < W && y >= 0 && y < H {
                self.buf[(y * W + x) as usize] = color;
            }
        }
        Ok(())
    }
}

impl OriginDimensions for Fb {
    fn size(&self) -> Size { Size::new(W as u32, H as u32) }
}

// ── Frame sync ──────────────────────────────────────────────────────────────

static FRAME_STATE: AtomicU8 = AtomicU8::new(0);

use core::cell::UnsafeCell;
struct SyncBuf(UnsafeCell<[Rgb565; PIXELS]>);
unsafe impl Sync for SyncBuf {}
static FRAMEBUF: SyncBuf = SyncBuf(UnsafeCell::new([Rgb565::BLACK; PIXELS]));

// Shared: number of beams that fired THIS frame (0 or more)
static LASER_FLASH: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

// ── Simple pseudo-random ────────────────────────────────────────────────────

fn prng(seed: u32) -> u32 {
    let mut x = seed;
    if x == 0 { x = 1; }
    x ^= x << 13;
    x ^= x >> 17;
    x ^= x << 5;
    x
}

// ── 5×7 bitmap font ────────────────────────────────────────────────────────

const GLYPH_W: u32 = 5;
const GLYPH_H: u32 = 7;
const GLYPH_GAP: u32 = 1;

fn glyph(ch: char) -> Option<[u8; 7]> {
    match ch {
        'A' | 'a' => Some([0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001]),
        'D' | 'd' => Some([0b11100, 0b10010, 0b10001, 0b10001, 0b10001, 0b10010, 0b11100]),
        'I' | 'i' => Some([0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110]),
        'K' | 'k' => Some([0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001]),
        ' ' => Some([0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000]),
        _ => None,
    }
}

fn is_text_pixel(px: i32, py: i32, ox: i32, oy: i32, s: u32, text: &[u8]) -> bool {
    let rx = px - ox;
    let ry = py - oy;
    if rx < 0 || ry < 0 { return false; }
    let rx = rx as u32;
    let ry = ry as u32;
    if ry >= GLYPH_H * s { return false; }
    let gr = ry / s;
    let cs = (GLYPH_W + GLYPH_GAP) * s;
    let ci = rx / cs;
    let wc = rx % cs;
    if ci >= text.len() as u32 || wc >= GLYPH_W * s { return false; }
    let row_idx = ci as usize;
    if row_idx >= text.len() { return false; }
    if let Some(rows) = glyph(text[row_idx] as char) {
        return (rows[gr as usize] >> (GLYPH_W - 1 - wc / s)) & 1 == 1;
    }
    false
}

// ── Laser beam data ─────────────────────────────────────────────────────────

const NUM_BEAMS: usize = 12;
const NAME: &[u8] = b"KADI";

struct Beam {
    // Start and end points
    x0: i32, y0: i32,
    x1: i32, y1: i32,
    birth: u32,
    active: bool,
}

struct LaserState {
    beams: [Beam; NUM_BEAMS],
    rng: u32,
}

impl LaserState {
    fn new() -> Self {
        const EMPTY: Beam = Beam { x0: 0, y0: 0, x1: 0, y1: 0, birth: 0, active: false };
        Self {
            beams: [EMPTY; NUM_BEAMS],
            rng: 54321,
        }
    }
}

// Pick a random point on a screen edge
fn random_edge_point(rng: &mut u32) -> (i32, i32) {
    *rng = prng(*rng);
    let edge = *rng % 4;
    *rng = prng(*rng);
    match edge {
        0 => ((*rng % W as u32) as i32, -5),       // top
        1 => ((*rng % W as u32) as i32, H + 5),    // bottom
        2 => (-5, (*rng % H as u32) as i32),        // left
        _ => (W + 5, (*rng % H as u32) as i32),     // right
    }
}

// ── Draw a line using Bresenham, additive green ─────────────────────────────

fn draw_laser_line(fb: &mut Fb, x0: i32, y0: i32, x1: i32, y1: i32, brightness: u8) {
    let dx = (x1 - x0).abs();
    let dy = -(y1 - y0).abs();
    let sx: i32 = if x0 < x1 { 1 } else { -1 };
    let sy: i32 = if y0 < y1 { 1 } else { -1 };
    let mut err = dx + dy;
    let mut cx = x0;
    let mut cy = y0;

    let g_core = (brightness as u32 * 63 / 255).min(63) as u8;
    let r_core = (brightness as u32 * 8 / 255).min(31) as u8;
    let g_glow = (g_core / 3).max(1);
    let r_glow = r_core / 4;

    let mut steps = 0i32;
    let max_steps = (W + H) * 2; // safety limit

    loop {
        // Draw core pixel
        if cx >= 0 && cx < W && cy >= 0 && cy < H {
            let idx = (cy * W + cx) as usize;
            let old = fb.buf[idx];
            let ng = (old.g() as u16 + g_core as u16).min(63) as u8;
            let nr = (old.r() as u16 + r_core as u16).min(31) as u8;
            fb.buf[idx] = Rgb565::new(nr, ng, 0);

            // Glow: 1px on each side perpendicular
            // Use simple offsets based on dominant direction
            let (gx, gy) = if dx.abs() > dy.abs() { (0, 1) } else { (1, 0) };
            for &(ox, oy) in &[(gx, gy), (-gx, -gy)] {
                let gxp = cx + ox;
                let gyp = cy + oy;
                if gxp >= 0 && gxp < W && gyp >= 0 && gyp < H {
                    let gi = (gyp * W + gxp) as usize;
                    let o = fb.buf[gi];
                    let ng2 = (o.g() as u16 + g_glow as u16).min(63) as u8;
                    let nr2 = (o.r() as u16 + r_glow as u16).min(31) as u8;
                    fb.buf[gi] = Rgb565::new(nr2, ng2, 0);
                }
            }
        }

        if cx == x1 && cy == y1 { break; }
        steps += 1;
        if steps > max_steps { break; }

        let e2 = 2 * err;
        if e2 >= dy {
            err += dy;
            cx += sx;
        }
        if e2 <= dx {
            err += dx;
            cy += sy;
        }
    }
}

// ── Main render ─────────────────────────────────────────────────────────────

fn render_lasers(fb: &mut Fb, frame: u32, state: &mut LaserState) {
    // Text layout — scale 10, centered
    let ts: u32 = 10;
    let tw = NAME.len() as i32 * (GLYPH_W as i32 + GLYPH_GAP as i32) * ts as i32
        - GLYPH_GAP as i32 * ts as i32;
    let tox = (W - tw) / 2;
    let toy = (H - (GLYPH_H * ts) as i32) / 2;

    // Clear to black
    for px in fb.buf.iter_mut() {
        *px = Rgb565::new(0, 0, 0);
    }

    // Update beams
    let mut new_fires: u32 = 0;
    for i in 0..NUM_BEAMS {
        if state.beams[i].active {
            let age = frame.wrapping_sub(state.beams[i].birth);
            if age > 15 {
                state.beams[i].active = false;
            }
        }

        if !state.beams[i].active {
            // Random chance to fire — roughly 1 in 6 per beam per frame
            state.rng = prng(state.rng);
            if state.rng % 12 == 0 {
                let (x0, y0) = random_edge_point(&mut state.rng);
                let (x1, y1) = random_edge_point(&mut state.rng);
                // Ensure start and end are on different edges (not too close)
                let dist_sq = (x1 - x0) * (x1 - x0) + (y1 - y0) * (y1 - y0);
                if dist_sq > 100 * 100 {
                    state.beams[i] = Beam {
                        x0, y0, x1, y1,
                        birth: frame,
                        active: true,
                    };
                    new_fires += 1;
                }
            }
        }
    }

    // Draw active beams (behind text)
    for i in 0..NUM_BEAMS {
        if !state.beams[i].active { continue; }
        let age = frame.wrapping_sub(state.beams[i].birth);

        // Brightness: flash bright on spawn, hold, then fade
        let brightness = if age < 3 {
            255u8
        } else if age < 8 {
            180
        } else {
            let fade = (15 - age).min(7) as u8;
            fade * 20
        };

        if brightness > 0 {
            draw_laser_line(fb,
                state.beams[i].x0, state.beams[i].y0,
                state.beams[i].x1, state.beams[i].y1,
                brightness,
            );
        }
    }

    // Draw KADI text ON TOP — bright green, opaque
    for y in 0..H {
        for x in 0..W {
            if is_text_pixel(x, y, tox, toy, ts, NAME) {
                let idx = (y * W + x) as usize;
                // Bright green text with slight glow
                fb.buf[idx] = Rgb565::new(4, 55, 3);
            }
        }
    }

    // Subtle particle sparks in background
    let mut spark_rng = frame.wrapping_mul(7919);
    for _ in 0..20 {
        spark_rng = prng(spark_rng);
        let sx = (spark_rng % W as u32) as i32;
        spark_rng = prng(spark_rng);
        let sy = (spark_rng % H as u32) as i32;
        spark_rng = prng(spark_rng);
        let bright = (spark_rng % 6) as u8;
        if !is_text_pixel(sx, sy, tox, toy, ts, NAME) {
            fb.buf[(sy * W + sx) as usize] = Rgb565::new(0, bright + 1, 0);
        }
    }

    // Signal LED task
    LASER_FLASH.store(new_fires, Ordering::Relaxed);
}

// ── Display blit (Core 1) ───────────────────────────────────────────────────

#[embassy_executor::task]
async fn display_blit_task(display: &'static mut Display<'static>) {
    info!("Display blit task on core 1");
    loop {
        if FRAME_STATE.load(Ordering::Acquire) == 1 {
            FRAME_STATE.store(2, Ordering::Release);
            let src: &[Rgb565; PIXELS] = unsafe { &*FRAMEBUF.0.get() };
            let area = Rectangle::new(Point::zero(), Size::new(W as u32, H as u32));
            display.fill_contiguous(&area, src.iter().copied()).unwrap();
            FRAME_STATE.store(0, Ordering::Release);
        } else {
            Timer::after(Duration::from_millis(1)).await;
        }
    }
}

// ── LED task — random individual green flashes ──────────────────────────────

#[embassy_executor::task]
async fn led_task(leds: &'static mut Leds<'static>) {
    info!("LED laser task started");
    let n = leds.len();
    // Per-LED flash countdown timers
    let mut led_timers: [u8; 32] = [0u8; 32]; // max 32 LEDs
    let mut rng: u32 = 99991;

    loop {
        let new_fires = LASER_FLASH.load(Ordering::Relaxed);

        // When lasers fire, randomly pick 1-3 LEDs to flash
        if new_fires > 0 {
            rng = prng(rng);
            let count = (rng % 3) + 1; // 1-3 LEDs
            for _ in 0..count {
                rng = prng(rng);
                let led_idx = (rng % n as u32) as usize;
                rng = prng(rng);
                let duration = 3 + (rng % 5) as u8; // 3-7 ticks
                led_timers[led_idx] = duration;
            }
        }

        // Update each LED independently
        for i in 0..n {
            if led_timers[i] > 0 {
                let bright = (led_timers[i] as u16 * 35).min(255) as u8;
                leds.set(i, Srgb::new(bright / 12, bright, 0));
                led_timers[i] -= 1;
            } else {
                leds.set(i, Srgb::new(0, 0, 0));
            }
        }

        leds.update().await;
        Timer::after(Duration::from_millis(40)).await;
    }
}

// ── Render task (Core 0) ────────────────────────────────────────────────────

#[embassy_executor::task]
async fn render_task() {
    info!("Render task on core 0");
    let mut frame: u32 = 0;
    let mut state = LaserState::new();

    loop {
        while FRAME_STATE.load(Ordering::Acquire) != 0 {
            Timer::after(Duration::from_millis(1)).await;
        }
        let fb_buf: &'static mut [Rgb565; PIXELS] = unsafe { &mut *FRAMEBUF.0.get() };
        let fb = &mut Fb { buf: fb_buf };
        render_lasers(fb, frame, &mut state);
        FRAME_STATE.store(1, Ordering::Release);
        frame = frame.wrapping_add(1);
    }
}

// ── Entry point ─────────────────────────────────────────────────────────────

#[esp_rtos::main]
async fn main(spawner: Spawner) -> ! {
    let peripherals = disobey2026badge::init();
    let resources = split_resources!(peripherals);

    esp_alloc::heap_allocator!(size: 64 * 1024);

    let timg0 = TimerGroup::new(peripherals.TIMG0);
    esp_rtos::start(timg0.timer0);

    use esp_hal::interrupt::software::SoftwareInterruptControl;
    let sw_ints = SoftwareInterruptControl::new(peripherals.SW_INTERRUPT);

    let core1_stack = mk_static!(
        esp_hal::system::Stack<8192>,
        esp_hal::system::Stack::new()
    );

    esp_rtos::start_second_core::<8192>(
        peripherals.CPU_CTRL,
        sw_ints.software_interrupt0,
        sw_ints.software_interrupt1,
        core1_stack,
        || {
            let executor = mk_static!(
                esp_rtos::embassy::Executor,
                esp_rtos::embassy::Executor::new()
            );
            executor.run(|spawner| {
                let display = mk_static!(Display<'static>, resources.display.into());
                let backlight = mk_static!(Backlight, resources.backlight.into());
                backlight.on();
                spawner.must_spawn(display_blit_task(display));
            });
        },
    );

    spawner.must_spawn(render_task());
    let leds = mk_static!(Leds<'static>, resources.leds.into());
    spawner.must_spawn(led_task(leds));

    loop { Timer::after(Duration::from_secs(600)).await; }
}
