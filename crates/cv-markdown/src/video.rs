//! A video in rendered Markdown: its first frame until it plays, then the frame its player took
//! ([`cv_media::video::player`]). A click on the picture, or Space while it has focus,
//! plays and pauses. While it is paused or hovered, a bar along its bottom holds play/pause, a
//! track to click or drag to a position, and the time.

use std::sync::Arc;

use cv_base::theme::{Palette, icons, style as tokens};
use cv_media::video::Clip;
use cv_media::video::player::{Players, Status};
use egui::{
    Align2, Color32, CursorIcon, FontId, Id, Key, Modifiers, Rect, Response, Sense, TextureId, Ui,
    WidgetInfo, pos2, vec2,
};

/// Height of the control bar (at zoom 1).
const BAR_HEIGHT: f32 = 32.0;
/// A video smaller than this shows no bar; a click still plays and pauses.
const BAR_MIN: egui::Vec2 = vec2(160.0, 72.0);
const TRACK_HEIGHT: f32 = 4.0;
/// Opacity of the shade behind the controls.
const SHADE_ALPHA: u8 = 150;

/// The first frame, drawn until the player has frames of its own, faded in by `opacity`.
#[derive(Clone, Copy)]
pub(super) struct Poster {
    pub texture: TextureId,
    pub opacity: f32,
}

/// What the viewer asked of the player this pass.
enum Command {
    Toggle,
    Seek(f64),
}

/// What the player is at this pass.
struct Playback {
    status: Status,
    position: f64,
    duration: f64,
}

/// Draws the video of widget `id` in `rect` (whose click and focus are `response`) and its
/// controls.
pub(super) fn show(
    ui: &Ui,
    rect: Rect,
    response: &Response,
    id: Id,
    clip: &Arc<Clip>,
    poster: Poster,
    palette: &Palette,
) {
    let ctx = ui.ctx().clone();
    let now = ctx.input(|input| input.time);
    let players = Players::of(&ctx);
    let (frame, repaint, playback) = players.with(id, clip, now, |player| {
        let (frame, repaint) = player.update(&ctx, now);
        let playback = Playback {
            status: player.status(),
            position: player.position(now),
            duration: player.duration(),
        };
        (frame, repaint, playback)
    });
    let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0));
    let tint = Color32::WHITE.gamma_multiply(poster.opacity);
    ui.painter().image(frame.unwrap_or(poster.texture), rect, uv, tint);

    let mut command = None;
    let hovered = ui.rect_contains_pointer(rect);
    if hovered {
        ctx.set_cursor_icon(CursorIcon::PointingHand);
    }
    let space = response.has_focus()
        && ui.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Space));
    if response.clicked() || space {
        command = Some(Command::Toggle);
    }
    if playback.status != Status::Playing {
        play_badge(ui, rect, playback.status, palette);
    }
    let shows_bar = (playback.status != Status::Playing || hovered)
        && rect.width() >= BAR_MIN.x
        && rect.height() >= BAR_MIN.y;
    if shows_bar && let Some(asked) = bar(ui, rect, id, &playback, palette) {
        command = Some(asked);
    }
    match command {
        Some(command) => {
            players.with(id, clip, now, |player| match command {
                Command::Toggle => player.toggle(now),
                Command::Seek(to) => player.seek(to, now),
            });
            ctx.request_repaint();
        }
        None => {
            if let Some(repaint) = repaint {
                ctx.request_repaint_after(repaint);
            }
        }
    }
}

/// The round play (or play again) mark in the middle of a video that is not playing.
fn play_badge(ui: &Ui, rect: Rect, status: Status, palette: &Palette) {
    let radius = (rect.height().min(rect.width()) * 0.18).clamp(12.0, 28.0);
    let painter = ui.painter();
    painter.circle_filled(rect.center(), radius, palette.accent);
    let icon = if status == Status::Ended { icons::ARROW_COUNTER_CLOCKWISE } else { icons::PLAY };
    let font = icons::font(radius, icons::Weight::Fill);
    painter.text(rect.center(), Align2::CENTER_CENTER, icon, font, palette.on_accent);
}

/// The control bar along the bottom of `rect`; what the viewer asked of it.
fn bar(ui: &Ui, rect: Rect, id: Id, playback: &Playback, palette: &Palette) -> Option<Command> {
    let bar = Rect::from_min_max(pos2(rect.left(), rect.bottom() - BAR_HEIGHT), rect.max);
    let painter = ui.painter();
    painter.rect_filled(bar, 0, Color32::from_black_alpha(SHADE_ALPHA));
    let ink = palette.on_accent;
    let mut command = None;

    let button = Rect::from_min_size(bar.min, vec2(BAR_HEIGHT, BAR_HEIGHT));
    let (icon, label) = match playback.status {
        Status::Playing => (icons::PAUSE, "Pause"),
        Status::Paused => (icons::PLAY, "Play"),
        Status::Ended => (icons::ARROW_COUNTER_CLOCKWISE, "Play again"),
    };
    let toggle = ui.interact(button, id.with("toggle"), Sense::click());
    toggle.widget_info(|| WidgetInfo::labeled(egui::Role::Button, true, label));
    if toggle.hovered() {
        painter.rect_filled(button.shrink(4.0), tokens::RADIUS_CONTROL, palette.hover);
    }
    painter.text(
        button.center(),
        Align2::CENTER_CENTER,
        icon,
        icons::font(icons::SIZE, icons::Weight::Fill),
        ink,
    );
    if toggle.clicked() {
        command = Some(Command::Toggle);
    }

    let time = format!("{} / {}", clock_text(playback.position), clock_text(playback.duration));
    let font = FontId::monospace(tokens::SMALL_SIZE);
    let time_at = pos2(bar.right() - tokens::SPACE_S, bar.center().y);
    let time_rect = painter.text(time_at, Align2::RIGHT_CENTER, time, font, ink);

    let track_area = Rect::from_min_max(
        pos2(button.right() + tokens::SPACE_XS, bar.top()),
        pos2(time_rect.left() - tokens::SPACE_M, bar.bottom()),
    );
    if playback.duration > 0.0 && track_area.width() > 24.0 {
        let share = (playback.position / playback.duration).clamp(0.0, 1.0) as f32;
        let seek = ui.interact(track_area, id.with("track"), Sense::click_and_drag());
        seek.widget_info(|| {
            WidgetInfo::slider(
                true,
                playback.position,
                format!("Position, {}", clock_text(playback.position)),
            )
        });
        let track =
            Rect::from_center_size(track_area.center(), vec2(track_area.width(), TRACK_HEIGHT));
        painter.rect_filled(track, TRACK_HEIGHT / 2.0, ink.gamma_multiply(0.35));
        let played = Rect::from_min_max(
            track.min,
            pos2(track.lerp_inside(vec2(share, 0.0)).x, track.bottom()),
        );
        painter.rect_filled(played, TRACK_HEIGHT / 2.0, palette.accent);
        if seek.hovered() || seek.dragged() {
            painter.circle_filled(pos2(played.right(), track.center().y), TRACK_HEIGHT * 1.5, ink);
        }
        if (seek.clicked() || seek.dragged())
            && let Some(pointer) = seek.interact_pointer_pos()
        {
            let share = f64::from(((pointer.x - track.left()) / track.width()).clamp(0.0, 1.0));
            command = Some(Command::Seek(share * playback.duration));
        }
    }
    command
}

/// `seconds` as `m:ss`, or `h:mm:ss` from an hour on.
fn clock_text(seconds: f64) -> String {
    let total = seconds.max(0.0).floor() as u64;
    let (hours, minutes, seconds) = (total / 3600, total / 60 % 60, total % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::clock_text;

    #[test]
    fn times_read_like_a_player_shows_them() {
        assert_eq!(clock_text(0.0), "0:00");
        assert_eq!(clock_text(65.9), "1:05");
        assert_eq!(clock_text(3_725.0), "1:02:05");
    }
}
