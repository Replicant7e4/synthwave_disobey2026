//! Synthwave / Outrun screen — standalone single-screen demo.
//!
//! Features: neon striped sun, sky gradient, perspective grid, road,
//! lamp posts, "REPLICANT" text, rainbow LEDs.
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

impl Fb {
    #[inline]
    fn set(&mut self, x: i32, y: i32, color: Rgb565) {
        if x >= 0 && x < W && y >= 0 && y < H {
            self.buf[(y * W + x) as usize] = color;
        }
    }
}

impl DrawTarget for Fb {
    type Color = Rgb565;
    type Error = core::convert::Infallible;
    fn draw_iter<I>(&mut self, pixels: I) -> Result<(), Self::Error>
    where I: IntoIterator<Item = Pixel<Self::Color>> {
        for Pixel(Point { x, y }, color) in pixels { self.set(x, y, color); }
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

// ── Sine LUT ────────────────────────────────────────────────────────────────

const SIN_Q: [i16; 65] = [
    0, 3, 6, 9, 12, 16, 19, 22, 25, 28, 31, 34, 37, 40, 43, 46,
    49, 51, 54, 57, 60, 62, 65, 67, 70, 72, 75, 77, 79, 81, 84, 86,
    88, 90, 92, 93, 95, 97, 99, 100, 102, 103, 105, 106, 107, 108, 110, 111,
    112, 113, 114, 114, 115, 116, 117, 117, 118, 118, 119, 119, 119, 120, 120, 120,
    120,
];

fn isin(angle: i32) -> i32 {
    let a = ((angle % 1024) + 1024) as u32 % 1024;
    let quadrant = a / 256;
    let idx = (a % 256) as usize;
    let i = idx * 64 / 256;
    let val = match quadrant {
        0 => SIN_Q[i],
        1 => SIN_Q[64 - i],
        2 => -SIN_Q[i],
        _ => -SIN_Q[64 - i],
    };
    val as i32
}

// ── 5×7 bitmap font ────────────────────────────────────────────────────────

const GLYPH_W: u32 = 5;
const GLYPH_H: u32 = 7;


fn glyph(ch: char) -> Option<[u8; 7]> {
    match ch {
        'A' | 'a' => Some([0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001]),
        'B' | 'b' => Some([0b11110, 0b10001, 0b10001, 0b11110, 0b10001, 0b10001, 0b11110]),
        'C' | 'c' => Some([0b01110, 0b10001, 0b10000, 0b10000, 0b10000, 0b10001, 0b01110]),
        'D' | 'd' => Some([0b11110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b11110]),
        'E' | 'e' => Some([0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111]),
        'F' | 'f' => Some([0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b10000]),
        'G' | 'g' => Some([0b01110, 0b10001, 0b10000, 0b10011, 0b10001, 0b10001, 0b01110]),
        'H' | 'h' => Some([0b10001, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001]),
        'I' | 'i' => Some([0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110]),
        'J' | 'j' => Some([0b00001, 0b00001, 0b00001, 0b00001, 0b10001, 0b10001, 0b01110]),
        'K' | 'k' => Some([0b10001, 0b10010, 0b10100, 0b11000, 0b10100, 0b10010, 0b10001]),
        'L' | 'l' => Some([0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111]),
        'M' | 'm' => Some([0b10001, 0b11011, 0b10101, 0b10001, 0b10001, 0b10001, 0b10001]),
        'N' | 'n' => Some([0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001]),
        'O' | 'o' => Some([0b01110, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110]),
        'P' | 'p' => Some([0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000]),
        'Q' | 'q' => Some([0b01110, 0b10001, 0b10001, 0b10001, 0b10101, 0b10010, 0b01101]),
        'R' | 'r' => Some([0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001]),
        'S' | 's' => Some([0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110]),
        'T' | 't' => Some([0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100]),
        'U' | 'u' => Some([0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01110]),
        'V' | 'v' => Some([0b10001, 0b10001, 0b10001, 0b10001, 0b10001, 0b01010, 0b00100]),
        'W' | 'w' => Some([0b10001, 0b10001, 0b10001, 0b10101, 0b10101, 0b11011, 0b10001]),
        'X' | 'x' => Some([0b10001, 0b10001, 0b01010, 0b00100, 0b01010, 0b10001, 0b10001]),
        'Y' | 'y' => Some([0b10001, 0b10001, 0b01010, 0b00100, 0b00100, 0b00100, 0b00100]),
        'Z' | 'z' => Some([0b11111, 0b00001, 0b00010, 0b00100, 0b01000, 0b10000, 0b11111]),
        '0' => Some([0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110]),
        '1' => Some([0b00100, 0b01100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110]),
        '2' => Some([0b01110, 0b10001, 0b00001, 0b00010, 0b00100, 0b01000, 0b11111]),
        '3' => Some([0b01110, 0b10001, 0b00001, 0b01110, 0b00001, 0b10001, 0b01110]),
        '4' => Some([0b00010, 0b00110, 0b01010, 0b10010, 0b11111, 0b00010, 0b00010]),
        '5' => Some([0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110]),
        '6' => Some([0b01110, 0b10000, 0b10000, 0b11110, 0b10001, 0b10001, 0b01110]),
        '7' => Some([0b11111, 0b00001, 0b00010, 0b00100, 0b00100, 0b00100, 0b00100]),
        '8' => Some([0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110]),
        '9' => Some([0b01110, 0b10001, 0b10001, 0b01111, 0b00001, 0b00001, 0b01110]),
        ' ' => Some([0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00000]),
        '.' => Some([0b00000, 0b00000, 0b00000, 0b00000, 0b00000, 0b00110, 0b00110]),
        '!' => Some([0b00100, 0b00100, 0b00100, 0b00100, 0b00000, 0b00000, 0b00100]),
        '-' => Some([0b00000, 0b00000, 0b00000, 0b11111, 0b00000, 0b00000, 0b00000]),
        _ => None,
    }
}

#[inline]
fn glyph_pixel_set(text: &[u8], ci: u32, gc: u32, gr: u32) -> bool {
    if ci >= text.len() as u32 || gc >= GLYPH_W || gr >= GLYPH_H { return false; }
    if let Some(rows) = glyph(text[ci as usize] as char) {
        return (rows[gr as usize] >> (GLYPH_W - 1 - gc)) & 1 == 1;
    }
    false
}

fn is_text_pixel(px: i32, py: i32, ox: i32, oy: i32, s: u32, gap: u32, text: &[u8]) -> bool {
    let rx = px - ox;
    let ry = py - oy;
    if rx < 0 || ry < 0 { return false; }
    let rx = rx as u32;
    let ry = ry as u32;
    if ry >= GLYPH_H * s { return false; }
    let gr = ry / s;
    let cs = GLYPH_W * s + gap;
    let ci = rx / cs;
    let wc = rx % cs;
    if ci >= text.len() as u32 || wc >= GLYPH_W * s { return false; }
    glyph_pixel_set(text, ci, wc / s, gr)
}

// ── Synthwave scene ─────────────────────────────────────────────────────────

const HORIZON_Y: i32 = 75;
const SUN_CX: i32 = 160; // W / 2
const SUN_CY: i32 = 70;  // slightly above horizon
const SUN_R: i32 = 45;
const ROAD_HALF: i32 = 20;
const NAME: &[u8] = b"Disobey2026";

struct SynthwaveState {
    scroll: i32,
}

impl SynthwaveState {
    fn new() -> Self { Self { scroll: 0 } }
}

fn render_synthwave(fb: &mut Fb, frame: u32, state: &mut SynthwaveState) {
    let f = (frame % 100000) as i32;
    state.scroll = (state.scroll + 6) % 100000;

    // Full-width text in front of sun — scale 5 = 325px wide (slight clip on edges)
    // Reduce gap to 3px to fit in 320px (Width = 305px)
    let ts: u32 = 5;
    let t_gap: u32 = 3;
    let tw = NAME.len() as i32 * (GLYPH_W as i32 * ts as i32 + t_gap as i32) - t_gap as i32;
    let tox = (W - tw) / 2;
    let toy = HORIZON_Y - (GLYPH_H * ts) as i32;

    for y in 0..H {
        let off = (y * W) as usize;
        for x in 0..W {
            let color = if y < HORIZON_Y {
                // Sky: dark purple/navy at top → hot magenta/pink near horizon
                // Reference: RGB(10,0,25) top → RGB(200,0,160) bottom
                let st = y * 256 / HORIZON_Y;
                let sky = Rgb565::new(
                    (1 + st * 24 / 256) as u8,   // 1→25 (dark→magenta red)
                    (0 + st * 2 / 256) as u8,    // 0→2 (very little green)
                    (3 + st * 17 / 256) as u8,   // 3→20 (purple→pink)
                );

                // Sun
                let dx = x - SUN_CX;
                let dy = y - SUN_CY;
                let dsq = dx * dx + dy * dy;
                let in_sun = dsq <= SUN_R * SUN_R;

                // Sun: smooth peach → salmon → coral → red gradient
                let sun_col = if in_sun {
                    let ft = (dy + SUN_R).max(0);
                    let diam = (SUN_R * 2).max(1);
                    // Thinner stripes, only in bottom third
                    let sp = (3 + ft / 8).max(1);
                    let gap = ft > diam * 2 / 3 && (ft % (sp * 2)) >= sp;
                    if gap {
                        None
                    } else {
                        // Direct ft/diam gradient (no intermediate normalization)
                        // Peach(31,50,16) top → Red(28,5,2) bottom
                        let r = (31 - ft * 3 / diam).max(0) as u8;
                        let g = (50 - ft * 45 / diam).max(0) as u8;
                        let b = (16 - ft * 14 / diam).max(0) as u8;
                        Some(Rgb565::new(r, g, b))
                    }
                } else {
                    None
                };

                // Text in front of everything
                let on_text = is_text_pixel(x, y, tox, toy, ts, t_gap, NAME);

                if on_text {
                    // Unicolor bright neon pink text everywhere
                    Rgb565::new(31, 0, 28)
                } else if let Some(sc) = sun_col {
                    sc
                } else {
                    sky
                }
            } else if y == HORIZON_Y {
                Rgb565::new(31, 0, 28)
            } else {
                // Ground
                let d = y - HORIZON_Y; // >= 1
                let z = 8000 / d;
                let zs = ((z + state.scroll).unsigned_abs()) % 100000;

                // Road
                let rw = ROAD_HALF + ROAD_HALF * 8 * d / (H - HORIZON_Y);
                let fc = (x - 160).abs(); // W/2 = 160
                let gridh = (zs % 40) < 2;
                let xw = (x - 160) * 200 / d;
                let _gridv = (xw.unsigned_abs() % 50) < 6;



                // Adaptive line width to fix aliasing (Box Filtering)
                // Ensure line width covers the sample step size
                let z_step = 8000 / (d * d).max(1);
                let x_step = 200 / d;
                
                let th_h = 2 + z_step;
                let th_v = 6 + x_step;

                if fc <= 1 {
                    // Dashed center line
                    if (zs / 20) % 2 == 0 { Rgb565::new(31, 63, 31) }
                    else { Rgb565::new(1, 3, 3) }
                } else if (fc - rw).abs() <= 1 {
                    // Road edge - Cyan (Synthwave style)
                    let v = isin(f * 4 + y * 3);
                    let p = ((v + 120) * 15 / 240).clamp(0, 30) as u8;
                    Rgb565::new(0, 33 + p, 31)
                } else if fc < rw {
                    if gridh { Rgb565::new(4, 8, 8) } else { Rgb565::new(1, 2, 2) }
                } else if d > 5 {
                    // Grid check with adaptive width
                    let is_grid_h = (zs % 40) < th_h as u32;
                    let is_grid_v = (xw.unsigned_abs() % 50) < th_v as u32;
                    let in_grid = is_grid_h || is_grid_v;

                    if in_grid {
                        let nv = isin(f * 3 + z);
                        let np = ((nv + 120) * 10 / 240).clamp(0, 10) as u8;
                        if is_grid_h && is_grid_v { Rgb565::new(15 + np, 0, 25) }
                        else if is_grid_h { Rgb565::new(0, 10 + np * 2, 20) }
                        else { Rgb565::new(5 + np, 0, 15) }
                    } else {
                        Rgb565::new(1, 2, 3)
                    }
                } else {
                    Rgb565::new(1, 2, 3)
                }
            };
            fb.buf[off + x as usize] = color;
        }
    }
}

// ── Display blit (Core 1) ───────────────────────────────────────────────────

#[embassy_executor::task]
async fn display_blit_task(display: &'static mut Display<'static>) {
    info!("Display blit task running on core 1");
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

// ── LED task ────────────────────────────────────────────────────────────────

#[embassy_executor::task]
async fn led_task(leds: &'static mut Leds<'static>) {
    info!("LED driving-forward task started");
    let n = leds.len();
    let mid = n / 2;
    let mut phase: u32 = 0;
    loop {
        // Driving forward: pulses stream outward from center
        // Like road dashes flying past the car
        for i in 0..n {
            let dist = if i < mid { (mid - 1 - i) as u32 } else { (i - mid) as u32 };
            // Create repeating pulse pattern streaming outward
            let wave = ((dist * 60 + phase) % 256) as u8;
            // Sharp pulse shape: bright near 0, dark otherwise
            let brightness = if wave < 40 { (40 - wave) as u8 } else { 0u8 };
            // Cyan/magenta alternating sides
            if i < mid {
                // Left side: cyan tint
                let r = brightness / 8;
                let g = brightness / 2;
                let b = brightness;
                leds.set(i, Srgb::new(r, g, b));
            } else {
                // Right side: magenta tint
                let r = brightness;
                let g = brightness / 8;
                let b = brightness / 2;
                leds.set(i, Srgb::new(r, g, b));
            }
        }
        leds.update().await;
        phase = (phase + 248) % 256; // subtract 8 to reverse direction
        Timer::after(Duration::from_millis(30)).await;
    }
}

// ── Render task (Core 0) ────────────────────────────────────────────────────

#[embassy_executor::task]
async fn render_task() {
    info!("Render task running on core 0");
    let mut frame: u32 = 0;
    let mut state = SynthwaveState::new();

    loop {
        while FRAME_STATE.load(Ordering::Acquire) != 0 {
            Timer::after(Duration::from_millis(1)).await;
        }
        let fb_buf: &'static mut [Rgb565; PIXELS] = unsafe { &mut *FRAMEBUF.0.get() };
        let fb = &mut Fb { buf: fb_buf };
        render_synthwave(fb, frame, &mut state);
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
