//! Fork: a slider with upstream's behaviour and the caller's own drawing,
//! for the waveform seek bar. A copy of `thin_slider`'s body; when upstream
//! changes how its slider behaves, copy the change here.

use super::*;

/// [`thin_slider`]'s behaviour, `size` large, drawn by `paint` with the
/// slider's rect, the value it shows and whether it is hovered, focused or
/// dragged.
pub fn painted_slider(
    ui: &mut Ui,
    id: egui::Id,
    label: &str,
    value: f32,
    size: Vec2,
    wheel_step: Option<f32>,
    paint: impl FnOnce(&egui::Painter, Rect, f32, bool),
) -> SliderEvent {
    let (_, rect) = ui.allocate_space(size);
    let response = ui.interact(rect, id, Sense::click_and_drag());
    let dragging_value = ui.data(|data| data.get_temp::<f32>(id));
    let pointer_value = response
        .interact_pointer_pos()
        .map(|pos| ((pos.x - rect.left()) / rect.width()).clamp(0.0, 1.0));
    let mut event = SliderEvent::None;
    // Preview on press, before egui's click-or-drag movement threshold.
    if (response.is_pointer_button_down_on() || response.drag_started() || response.dragged())
        && let Some(v) = pointer_value
    {
        ui.data_mut(|data| data.insert_temp(id, v));
        event = SliderEvent::Dragging(v);
    }
    if response.drag_stopped() {
        let v = dragging_value.or(pointer_value).unwrap_or(value);
        ui.data_mut(|data| data.remove::<f32>(id));
        event = SliderEvent::Committed(v);
    } else if response.clicked()
        && let Some(v) = pointer_value
    {
        ui.data_mut(|data| data.remove::<f32>(id));
        event = SliderEvent::Committed(v);
    }
    if let Some(step) = wheel_step
        && response.enabled()
    {
        let notches = wheel_notches(ui, &response);
        if notches != 0 {
            event = SliderEvent::Committed((value + step * notches as f32).clamp(0.0, 1.0));
        }
    }
    let step = wheel_step.unwrap_or(0.01);
    let focused = response.has_focus();
    if focused {
        ui.memory_mut(|memory| {
            memory.set_focus_lock_filter(
                response.id,
                egui::EventFilter {
                    horizontal_arrows: true,
                    ..Default::default()
                },
            )
        });
    }
    if response.enabled() {
        ui.input(|input| {
            use egui::accesskit::{Action, ActionData};
            let mut change = input.num_accesskit_action_requests(response.id, Action::Increment)
                as i32
                - input.num_accesskit_action_requests(response.id, Action::Decrement) as i32;
            if focused {
                change += input.num_presses(egui::Key::ArrowRight) as i32
                    - input.num_presses(egui::Key::ArrowLeft) as i32;
            }
            if change != 0 {
                event = SliderEvent::Committed((value + change as f32 * step).clamp(0.0, 1.0));
            }
            for request in input.accesskit_action_requests(response.id, Action::SetValue) {
                if let Some(ActionData::NumericValue(value)) = request.data
                    && value.is_finite()
                {
                    event = SliderEvent::Committed((value / 100.0).clamp(0.0, 1.0) as f32);
                }
            }
        });
    }
    let shown = match &event {
        SliderEvent::Dragging(v) => *v,
        SliderEvent::Committed(v) => *v,
        SliderEvent::None => dragging_value.unwrap_or(value),
    };
    response
        .widget_info(|| egui::WidgetInfo::slider(ui.is_enabled(), f64::from(shown) * 100.0, label));
    ui.ctx().accesskit_node_builder(response.id, |node| {
        use egui::accesskit::Action;
        node.set_min_numeric_value(0.0);
        node.set_max_numeric_value(100.0);
        node.set_numeric_value_step(f64::from(step) * 100.0);
        node.add_action(Action::SetValue);
        if shown > 0.0 {
            node.add_action(Action::Decrement);
        }
        if shown < 1.0 {
            node.add_action(Action::Increment);
        }
    });
    if ui.is_rect_visible(rect) {
        let active = response.hovered()
            || response.has_focus()
            || response.dragged()
            || dragging_value.is_some();
        paint(ui.painter(), rect, shown, active);
    }
    event
}
