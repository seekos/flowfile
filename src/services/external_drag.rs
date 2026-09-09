use gpui::{Pixels, Point, Window};
use objc2::{ClassType, DeclaredClass, declare_class, msg_send_id, mutability};
use objc2_app_kit::{
    NSApplication, NSDragOperation, NSDraggingContext, NSDraggingFormation, NSDraggingItem,
    NSDraggingSession, NSDraggingSource, NSView, NSWorkspace,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use std::{
    cell::{Cell, RefCell},
    path::PathBuf,
};

declare_class!(
    struct FlowFileDraggingSource;

    // SAFETY: NSObject has no subclassing requirements, and AppKit only uses
    // dragging sources on the main thread.
    unsafe impl ClassType for FlowFileDraggingSource {
        type Super = NSObject;
        type Mutability = mutability::MainThreadOnly;
        const NAME: &'static str = "FlowFileDraggingSource";
    }

    impl DeclaredClass for FlowFileDraggingSource {}

    unsafe impl NSObjectProtocol for FlowFileDraggingSource {}

    unsafe impl NSDraggingSource for FlowFileDraggingSource {
        #[method(draggingSession:sourceOperationMaskForDraggingContext:)]
        fn source_operation_mask(
            &self,
            _session: &NSDraggingSession,
            _context: NSDraggingContext,
        ) -> NSDragOperation {
            NSDragOperation::Copy | NSDragOperation::Move | NSDragOperation::Link
        }

        #[method(draggingSession:endedAtPoint:operation:)]
        fn dragging_ended(
            &self,
            _session: &NSDraggingSession,
            _screen_point: NSPoint,
            _operation: NSDragOperation,
        ) {
            finish_drag();
        }
    }
);

thread_local! {
    // NSDraggingSession doesn't retain its source. Keep it alive until AppKit
    // shuts down; the source is stateless and can be reused for every drag.
    static DRAG_SOURCE: RefCell<Option<objc2::rc::Retained<FlowFileDraggingSource>>> = const { RefCell::new(None) };
    static DRAG_ACTIVE: Cell<bool> = const { Cell::new(false) };
    static DRAG_END_CALLBACK: RefCell<Option<Box<dyn FnOnce()>>> = const { RefCell::new(None) };
}

/// Starts a native macOS file drag for the supplied paths.
///
/// This must be called from a mouse-drag event on AppKit's main thread.
pub fn begin_external_file_drag<F>(
    paths: &[PathBuf],
    pointer_start: Point<Pixels>,
    visual_origin: Point<Pixels>,
    pointer_current: Point<Pixels>,
    window: &Window,
    on_end: F,
) -> bool
where
    F: FnOnce() + 'static,
{
    let existing_paths: Vec<_> = paths.iter().filter(|path| path.exists()).collect();
    if existing_paths.is_empty() || DRAG_ACTIVE.with(Cell::get) {
        return false;
    }

    let Some(main_thread) = MainThreadMarker::new() else {
        return false;
    };
    let Ok(window_handle) = HasWindowHandle::window_handle(window) else {
        return false;
    };
    let RawWindowHandle::AppKit(handle) = window_handle.as_raw() else {
        return false;
    };

    // SAFETY: GPUI's raw handle points to its live NSView, this function runs
    // synchronously from that view's mouse event, and all AppKit calls stay on
    // the main thread.
    unsafe {
        let view = &*(handle.ns_view.as_ptr().cast::<NSView>());
        let application = NSApplication::sharedApplication(main_thread);
        let Some(event) = application.currentEvent() else {
            return false;
        };

        let workspace = NSWorkspace::sharedWorkspace();
        let location = view.convertPoint_fromView(event.locationInWindow(), None);
        let preview_center = dragging_preview_center(
            location,
            view.isFlipped(),
            pointer_start,
            visual_origin,
            pointer_current,
        );
        let item_count = existing_paths.len();
        let icon_size = if item_count > 1 { 56.0 } else { 64.0 };
        let mut dragging_items = Vec::with_capacity(existing_paths.len());

        for (index, path) in existing_paths.into_iter().enumerate() {
            let path_string = NSString::from_str(&path.to_string_lossy());
            let file_url = NSURL::fileURLWithPath(&path_string);
            let writer = objc2::runtime::ProtocolObject::from_ref(&*file_url);
            let dragging_item =
                NSDraggingItem::initWithPasteboardWriter(NSDraggingItem::alloc(), writer);
            let icon = workspace.iconForFile(&path_string);
            let offset = (index.min(6) as f64) * 4.0;
            let frame = NSRect::new(
                NSPoint::new(
                    preview_center.x - icon_size / 2.0 + offset,
                    preview_center.y - icon_size / 2.0 - offset,
                ),
                NSSize::new(icon_size, icon_size),
            );
            dragging_item.setDraggingFrame_contents(frame, Some(&icon));
            dragging_items.push(dragging_item);
        }

        let items = NSArray::from_vec(dragging_items);
        let source = DRAG_SOURCE.with(|stored| {
            if let Some(source) = stored.borrow().clone() {
                return source;
            }
            let source = main_thread.alloc::<FlowFileDraggingSource>();
            let source = source.set_ivars(());
            let source: objc2::rc::Retained<FlowFileDraggingSource> =
                msg_send_id![super(source), init];
            *stored.borrow_mut() = Some(source.clone());
            source
        });
        let source_protocol = objc2::runtime::ProtocolObject::from_ref(&*source);
        DRAG_ACTIVE.with(|active| active.set(true));
        DRAG_END_CALLBACK.with(|callback| *callback.borrow_mut() = Some(Box::new(on_end)));

        let session =
            view.beginDraggingSessionWithItems_event_source(&items, &event, source_protocol);
        session.setDraggingFormation(if item_count > 1 {
            NSDraggingFormation::Stack
        } else {
            NSDraggingFormation::Default
        });
        true
    }
}

fn finish_drag() {
    DRAG_ACTIVE.with(|active| active.set(false));
    DRAG_END_CALLBACK.with(|callback| {
        if let Some(on_end) = callback.borrow_mut().take() {
            on_end();
        }
    });
}

fn dragging_preview_center(
    appkit_pointer_current: NSPoint,
    view_is_flipped: bool,
    pointer_start: Point<Pixels>,
    visual_origin: Point<Pixels>,
    pointer_current: Point<Pixels>,
) -> NSPoint {
    let translated_origin = visual_origin + (pointer_current - pointer_start);
    let offset = translated_origin - pointer_current;
    let x = appkit_pointer_current.x + f64::from(offset.x);
    let y_offset = f64::from(offset.y);
    let y = if view_is_flipped {
        appkit_pointer_current.y + y_offset
    } else {
        appkit_pointer_current.y - y_offset
    };
    NSPoint::new(x, y)
}

/// Clears FlowFile's logical drag state as soon as the initiating button is released.
/// AppKit also invokes the dragging-source callback, so this is an idempotent UI fallback.
pub fn end_external_file_drag() {
    finish_drag();
}

#[cfg(test)]
mod tests {
    use super::{DRAG_END_CALLBACK, dragging_preview_center, end_external_file_drag};
    use gpui::{point, px};
    use objc2_foundation::NSPoint;
    use std::{cell::Cell, rc::Rc};

    #[test]
    fn drag_preview_preserves_the_pointer_offset_from_the_source_visual() {
        let center = dragging_preview_center(
            NSPoint::new(240.0, 180.0),
            false,
            point(px(100.0), px(120.0)),
            point(px(88.0), px(96.0)),
            point(px(140.0), px(150.0)),
        );

        assert_eq!(center.x, 228.0);
        assert_eq!(center.y, 204.0);
    }

    #[test]
    fn drag_end_callback_only_runs_once() {
        let calls = Rc::new(Cell::new(0));
        let calls_for_callback = calls.clone();
        DRAG_END_CALLBACK.with(|callback| {
            *callback.borrow_mut() = Some(Box::new(move || {
                calls_for_callback.set(calls_for_callback.get() + 1);
            }));
        });

        end_external_file_drag();
        end_external_file_drag();

        assert_eq!(calls.get(), 1);
    }
}
