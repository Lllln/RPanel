mod mouse;
mod popup;

pub use mouse::{MouseListener, Trigger};
pub use popup::Popup;

pub fn configure(options: &mut eframe::NativeOptions) {
    use winit::platform::macos::{ActivationPolicy, EventLoopBuilderExtMacOS};

    options.viewport = std::mem::take(&mut options.viewport).with_transparent(true);
    options.event_loop_builder = Some(Box::new(|builder| {
        builder
            .with_activation_policy(ActivationPolicy::Accessory)
            .with_activate_ignoring_other_apps(false)
            .with_default_menu(false);
    }));
}
