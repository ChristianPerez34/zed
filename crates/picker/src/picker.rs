use editor::Editor;
use gpui::{
    elements::{
        ChildView, Flex, Label, MouseEventHandler, ParentElement, ScrollTarget, UniformList,
        UniformListState,
    },
    geometry::vector::{vec2f, Vector2F},
    keymap,
    platform::CursorStyle,
    AnyViewHandle, AppContext, Axis, Element, ElementBox, Entity, MouseButton, MouseState,
    MutableAppContext, RenderContext, Task, View, ViewContext, ViewHandle, WeakViewHandle,
};
use menu::{Cancel, Confirm, SelectFirst, SelectIndex, SelectLast, SelectNext, SelectPrev};
use settings::Settings;
use std::cmp;

pub struct Picker<D: PickerDelegate> {
    delegate: WeakViewHandle<D>,
    query_editor: ViewHandle<Editor>,
    list_state: UniformListState,
    max_size: Vector2F,
    confirmed: bool,
}

pub trait PickerDelegate: View {
    fn match_count(&self) -> usize;
    fn selected_index(&self) -> usize;
    fn set_selected_index(&mut self, ix: usize, cx: &mut ViewContext<Self>);
    fn update_matches(&mut self, query: String, cx: &mut ViewContext<Self>) -> Task<()>;
    fn confirm(&mut self, cx: &mut ViewContext<Self>);
    fn dismiss(&mut self, cx: &mut ViewContext<Self>);
    fn render_match(
        &self,
        ix: usize,
        state: MouseState,
        selected: bool,
        cx: &AppContext,
    ) -> ElementBox;
    fn center_selection_after_match_updates(&self) -> bool {
        false
    }
}

impl<D: PickerDelegate> Entity for Picker<D> {
    type Event = ();
}

impl<D: PickerDelegate> View for Picker<D> {
    fn ui_name() -> &'static str {
        "Picker"
    }

    fn render(&mut self, cx: &mut RenderContext<Self>) -> gpui::ElementBox {
        let settings = cx.global::<Settings>();
        let container_style = settings.theme.picker.container;
        let delegate = self.delegate.clone();
        let match_count = if let Some(delegate) = delegate.upgrade(cx.app) {
            delegate.read(cx).match_count()
        } else {
            0
        };

        Flex::new(Axis::Vertical)
            .with_child(
                ChildView::new(&self.query_editor)
                    .contained()
                    .with_style(settings.theme.picker.input_editor.container)
                    .boxed(),
            )
            .with_child(
                if match_count == 0 {
                    Label::new(
                        "No matches".into(),
                        settings.theme.picker.empty.label.clone(),
                    )
                    .contained()
                    .with_style(settings.theme.picker.empty.container)
                } else {
                    UniformList::new(
                        self.list_state.clone(),
                        match_count,
                        cx,
                        move |this, mut range, items, cx| {
                            let delegate = this.delegate.upgrade(cx).unwrap();
                            let selected_ix = delegate.read(cx).selected_index();
                            range.end = cmp::min(range.end, delegate.read(cx).match_count());
                            items.extend(range.map(move |ix| {
                                MouseEventHandler::new::<D, _, _>(ix, cx, |state, cx| {
                                    delegate
                                        .read(cx)
                                        .render_match(ix, state, ix == selected_ix, cx)
                                })
                                .on_down(MouseButton::Left, move |_, cx| {
                                    cx.dispatch_action(SelectIndex(ix))
                                })
                                .with_cursor_style(CursorStyle::PointingHand)
                                .boxed()
                            }));
                        },
                    )
                    .contained()
                    .with_margin_top(6.0)
                }
                .flex(1., false)
                .boxed(),
            )
            .contained()
            .with_style(container_style)
            .constrained()
            .with_max_width(self.max_size.x())
            .with_max_height(self.max_size.y())
            .named("picker")
    }

    fn keymap_context(&self, _: &AppContext) -> keymap::Context {
        let mut cx = Self::default_keymap_context();
        cx.set.insert("menu".into());
        cx
    }

    fn on_focus_in(&mut self, _: AnyViewHandle, cx: &mut ViewContext<Self>) {
        if cx.is_self_focused() {
            cx.focus(&self.query_editor);
        }
    }
}

impl<D: PickerDelegate> Picker<D> {
    pub fn init(cx: &mut MutableAppContext) {
        cx.add_action(Self::select_first);
        cx.add_action(Self::select_last);
        cx.add_action(Self::select_next);
        cx.add_action(Self::select_prev);
        cx.add_action(Self::select_index);
        cx.add_action(Self::confirm);
        cx.add_action(Self::cancel);
    }

    pub fn new(delegate: WeakViewHandle<D>, cx: &mut ViewContext<Self>) -> Self {
        let query_editor = cx.add_view(|cx| {
            Editor::single_line(Some(|theme| theme.picker.input_editor.clone()), cx)
        });
        cx.subscribe(&query_editor, Self::on_query_editor_event)
            .detach();
        let this = Self {
            query_editor,
            list_state: Default::default(),
            delegate,
            max_size: vec2f(540., 420.),
            confirmed: false,
        };
        cx.defer(|this, cx| {
            if let Some(delegate) = this.delegate.upgrade(cx) {
                cx.observe(&delegate, |_, _, cx| cx.notify()).detach();
                this.update_matches(String::new(), cx)
            }
        });
        this
    }

    pub fn with_max_size(mut self, width: f32, height: f32) -> Self {
        self.max_size = vec2f(width, height);
        self
    }

    pub fn query(&self, cx: &AppContext) -> String {
        self.query_editor.read(cx).text(cx)
    }

    fn on_query_editor_event(
        &mut self,
        _: ViewHandle<Editor>,
        event: &editor::Event,
        cx: &mut ViewContext<Self>,
    ) {
        match event {
            editor::Event::BufferEdited { .. } => self.update_matches(self.query(cx), cx),
            editor::Event::Blurred if !self.confirmed => {
                if let Some(delegate) = self.delegate.upgrade(cx) {
                    delegate.update(cx, |delegate, cx| {
                        delegate.dismiss(cx);
                    })
                }
            }
            _ => {}
        }
    }

    pub fn update_matches(&mut self, query: String, cx: &mut ViewContext<Self>) {
        if let Some(delegate) = self.delegate.upgrade(cx) {
            let update = delegate.update(cx, |d, cx| d.update_matches(query, cx));
            cx.spawn(|this, mut cx| async move {
                update.await;
                this.update(&mut cx, |this, cx| {
                    if let Some(delegate) = this.delegate.upgrade(cx) {
                        let delegate = delegate.read(cx);
                        let index = delegate.selected_index();
                        let target = if delegate.center_selection_after_match_updates() {
                            ScrollTarget::Center(index)
                        } else {
                            ScrollTarget::Show(index)
                        };
                        this.list_state.scroll_to(target);
                        cx.notify();
                    }
                });
            })
            .detach()
        }
    }

    pub fn select_first(&mut self, _: &SelectFirst, cx: &mut ViewContext<Self>) {
        if let Some(delegate) = self.delegate.upgrade(cx) {
            let index = 0;
            delegate.update(cx, |delegate, cx| delegate.set_selected_index(0, cx));
            self.list_state.scroll_to(ScrollTarget::Show(index));
            cx.notify();
        }
    }

    pub fn select_index(&mut self, action: &SelectIndex, cx: &mut ViewContext<Self>) {
        if let Some(delegate) = self.delegate.upgrade(cx) {
            let index = action.0;
            self.confirmed = true;
            delegate.update(cx, |delegate, cx| {
                delegate.set_selected_index(index, cx);
                delegate.confirm(cx);
            });
        }
    }

    pub fn select_last(&mut self, _: &SelectLast, cx: &mut ViewContext<Self>) {
        if let Some(delegate) = self.delegate.upgrade(cx) {
            let index = delegate.update(cx, |delegate, cx| {
                let match_count = delegate.match_count();
                let index = if match_count > 0 { match_count - 1 } else { 0 };
                delegate.set_selected_index(index, cx);
                index
            });
            self.list_state.scroll_to(ScrollTarget::Show(index));
            cx.notify();
        }
    }

    pub fn select_next(&mut self, _: &SelectNext, cx: &mut ViewContext<Self>) {
        if let Some(delegate) = self.delegate.upgrade(cx) {
            let index = delegate.update(cx, |delegate, cx| {
                let mut selected_index = delegate.selected_index();
                if selected_index + 1 < delegate.match_count() {
                    selected_index += 1;
                    delegate.set_selected_index(selected_index, cx);
                }
                selected_index
            });
            self.list_state.scroll_to(ScrollTarget::Show(index));
            cx.notify();
        }
    }

    pub fn select_prev(&mut self, _: &SelectPrev, cx: &mut ViewContext<Self>) {
        if let Some(delegate) = self.delegate.upgrade(cx) {
            let index = delegate.update(cx, |delegate, cx| {
                let mut selected_index = delegate.selected_index();
                if selected_index > 0 {
                    selected_index -= 1;
                    delegate.set_selected_index(selected_index, cx);
                }
                selected_index
            });
            self.list_state.scroll_to(ScrollTarget::Show(index));
            cx.notify();
        }
    }

    fn confirm(&mut self, _: &Confirm, cx: &mut ViewContext<Self>) {
        if let Some(delegate) = self.delegate.upgrade(cx) {
            self.confirmed = true;
            delegate.update(cx, |delegate, cx| delegate.confirm(cx));
        }
    }

    fn cancel(&mut self, _: &Cancel, cx: &mut ViewContext<Self>) {
        if let Some(delegate) = self.delegate.upgrade(cx) {
            delegate.update(cx, |delegate, cx| delegate.dismiss(cx));
        }
    }
}
