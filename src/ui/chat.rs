//! ChatAI GPUI view — the main chat panel.
//!
//! Ported from `chat-ai/src/chat.rs` with two small adjustments:
//!   * module paths now point at `crate::ui::...` instead of `crate::...`;
//!   * the agent services module is the new library-backed one in
//!     `crate::ui::services::agent`.
//!
//! Everything else (TitleBar, message list, input form, model select,
//! attachment ingest) is unchanged so the look & feel of chat-ai is
//! preserved.

use crate::agent::TurnEvent;
use crate::core::types::ToolResultContent;
use crate::tools::UserQuestionEvent;
use crate::ui::{
    AskModal, PermissionModal, PermissionRequest, Settings, SettingsPanel,
    handler::{handle_incoming, handle_outgoing},
    services::agent::{AgentRequest, AgentResponse, MessageRole, UiMessage},
    theme::change_color_mode,
};
use async_channel::{Sender, unbounded};
use gpui::{
    AnyElement, App, AppContext as _, ClickEvent, Context, Div, Entity, IntoElement, ListAlignment,
    ListState, ParentElement as _, PathPromptOptions, Render, SharedString, Styled as _, Window,
    div, list, prelude::FluentBuilder as _, px,
};
use gpui_component::{
    ActiveTheme as _, Icon, Sizable as _, StyledExt as _, ThemeMode, TitleBar,
    alert::Alert,
    button::*,
    divider::Divider,
    h_flex,
    input::{Input, InputEvent, InputState},
    label::Label,
    text::TextView,
};
use std::path::PathBuf;
use std::time::Instant;

pub struct MessageState {
    messages: Vec<UiMessage>,
}

pub struct ChatAI {
    text_input: Entity<InputState>,
    message_state: Entity<MessageState>,
    list_state: ListState,
    request_tx: Sender<AgentRequest>,
    attached_files: Vec<PathBuf>,
    is_loading: bool,
    has_api_key: bool,
    /// Persisted settings (api key, base url, model). Loaded at startup,
    /// updated whenever the user saves the settings panel.
    settings: Settings,
    /// Whether the settings panel is currently shown.
    show_settings: bool,
    /// The settings panel view. Only rendered when `show_settings` is true.
    settings_panel: Option<Entity<SettingsPanel>>,

    /// Accumulated assistant text for the in-flight turn. Deltas append here
    /// and are flushed to the message list (throttled) by
    /// [`Self::update_streaming_message`].
    streaming_text: String,
    /// Timestamp of the last streaming render — used to throttle UI updates
    /// to ~20 fps (50 ms) while deltas arrive.
    last_render_at: Option<Instant>,
    /// Overlay shown when the agent requests tool-call permission.
    permission_modal: Option<Entity<PermissionModal>>,
    /// Overlay shown when the `AskUserQuestion` tool prompts the user.
    ask_modal: Option<Entity<AskModal>>,
    /// A pending `AskUserQuestion` event awaiting a `&mut Window` to be
    /// dispatched to [`AskModal::show`] via `window.defer` during render.
    pending_ask: Option<UserQuestionEvent>,
}

impl ChatAI {
    pub fn view(window: &mut Window, cx: &mut App) -> Entity<ChatAI> {
        cx.new(|cx| ChatAI::new(window, cx))
    }
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        // Load persisted settings (api key, base url, model). Falls back to
        // the `ANTHROPIC_API_KEY` env var on first launch.
        let settings = Settings::load().unwrap_or_default();
        let has_api_key = settings.has_api_key();

        /*
         * Create a channel
         * Spawn on the background and block there, send messages over the channel
         * Spawn on the foreground and listen to the channel
         * As events come in, send them over the channel to the main thread to be processed
         */
        let (response_tx, response_rx) = unbounded::<AgentResponse>();
        let (request_tx, request_rx) = unbounded::<AgentRequest>();

        // Spawn the agent message handler in background
        cx.background_executor()
            .spawn(handle_outgoing(request_rx, response_tx))
            .detach();

        // Spawn foreground task to handle incoming responses from agent
        // detaching let's it run to execution
        cx.spawn(async move |this, cx| {
            handle_incoming(this, response_rx, cx).await;
        })
        .detach();

        let list_state = ListState::new(0, ListAlignment::Bottom, px(200.));

        // Initialize state with empty messages
        let message_state = cx.new(|_cx| MessageState { messages: vec![] });

        // When messages are updated, update our list
        cx.observe(&message_state, |this: &mut ChatAI, _event, cx| {
            let items = this.message_state.read(cx).messages.clone();
            this.list_state = ListState::new(items.len(), ListAlignment::Bottom, px(20.));
            cx.notify();
        })
        .detach();

        let text_input = cx.new(|cx| {
            InputState::new(window, cx)
                .auto_grow(1, 3)
                .soft_wrap(true)
                .placeholder("Ask me anything")
        });

        // Send on plain Enter; Shift+Enter (the input crate's
        // `secondary-enter` binding) keeps its default behaviour of
        // inserting a newline. The `secondary` flag on `PressEnter` is
        // `true` for modifier+Enter bindings.
        //
        // `cx.subscribe` only hands us `&mut Context<Self>`, with no
        // `&mut Window`. We dispatch through `submit_text_from_enter`,
        // which performs the read-only steps (text read, file take,
        // channel send) synchronously, then schedules a `Window::defer`
        // callback to clear the input on the next render tick.
        cx.subscribe(&text_input, |this, _input, event, cx| {
            if let InputEvent::PressEnter { secondary: false } = event {
                this.submit_text_from_enter(cx);
            }
        })
        .detach();

        // Modal entities — kept mounted for the lifetime of the view and
        // rendered as overlays when active. Created here (rather than lazily)
        // so `show_permission_modal` / `show_ask_modal` always have a target.
        let permission_modal = cx.new(|cx| PermissionModal::new(window, cx));
        let ask_modal = cx.new(|cx| AskModal::new(window, cx));

        // Push the loaded settings to the background agent so it boots with
        // the right provider / api_key / base_url / model. This is a one-shot
        // config send on startup — no need to wait for an ack.
        let _ = request_tx.try_send(AgentRequest::SetProvider(settings.provider.clone()));
        if has_api_key {
            let _ = request_tx.try_send(AgentRequest::SetApiConfig {
                api_key: settings.api_key.clone(),
                base_url: settings.effective_base_url(),
            });
        }
        let _ = request_tx.try_send(AgentRequest::SetModel(settings.model.clone()));
        let _ = request_tx.try_send(AgentRequest::SetWorkingDir(settings.working_dir.clone()));

        Self {
            text_input,
            message_state,
            list_state,
            request_tx,
            is_loading: false,
            has_api_key,
            settings,
            show_settings: false,
            settings_panel: None,
            attached_files: vec![],
            streaming_text: String::new(),
            last_render_at: None,
            permission_modal: Some(permission_modal),
            ask_modal: Some(ask_modal),
            pending_ask: None,
        }
    }

    pub fn add_message(&mut self, message: UiMessage, cx: &mut Context<Self>) {
        cx.update_entity(&self.message_state, |state, cx| {
            state.messages.push(message);
            cx.notify();
        });
    }

    pub fn set_loading(&mut self, loading: bool, cx: &mut Context<Self>) {
        self.is_loading = loading;
        cx.notify();
    }

    /// Render a [`TurnEvent`] streaming from the background agent into the chat
    /// view. Text deltas accumulate in `streaming_text` (throttled); tool
    /// calls / results are surfaced as system messages; `Done` / `Failed` /
    /// `Cancelled` flush the streaming buffer and clear the loading state.
    pub fn handle_turn_event(&mut self, ev: TurnEvent, cx: &mut Context<Self>) {
        match ev {
            TurnEvent::TextDelta { text } => {
                self.streaming_text.push_str(&text);
                // Throttle: don't re-render more often than every 50 ms.
                let now = Instant::now();
                let should_render = self
                    .last_render_at
                    .map(|t| now.duration_since(t).as_millis() > 50)
                    .unwrap_or(true);
                if should_render {
                    self.last_render_at = Some(now);
                    self.update_streaming_message(cx);
                }
            }
            TurnEvent::ToolUseStart { name, .. } => {
                self.add_message(
                    UiMessage::system(format!("🔧 调用工具: {}...", name)),
                    cx,
                );
            }
            TurnEvent::ToolUseDelta { .. } => {
                // Input JSON deltas are not rendered inline.
            }
            TurnEvent::ToolEnd {
                result, is_error, ..
            } => {
                let text = match &result {
                    ToolResultContent::Text(t) => t.clone(),
                    ToolResultContent::Blocks(_) => "[structured result]".to_string(),
                };
                let prefix = if is_error { "✗" } else { "✓" };
                self.add_message(
                    UiMessage::system(format!("{} 工具返回: {}", prefix, text)),
                    cx,
                );
            }
            TurnEvent::Done { .. } => {
                self.update_streaming_message(cx);
                self.streaming_text.clear();
                self.last_render_at = None;
                self.set_loading(false, cx);
            }
            TurnEvent::Failed { error } => {
                self.add_message(UiMessage::error(format!("{}", error)), cx);
                self.streaming_text.clear();
                self.last_render_at = None;
                self.set_loading(false, cx);
            }
            TurnEvent::Cancelled => {
                if !self.streaming_text.is_empty() {
                    self.streaming_text.push_str(" (已取消)");
                    self.update_streaming_message(cx);
                }
                self.streaming_text.clear();
                self.last_render_at = None;
                self.set_loading(false, cx);
            }
        }
    }

    /// Flush `streaming_text` into the message list. If the last message is an
    /// assistant message it is updated in place; otherwise a new assistant
    /// message is appended. No-op when the buffer is empty.
    fn update_streaming_message(&mut self, cx: &mut Context<Self>) {
        if self.streaming_text.is_empty() {
            return;
        }
        let text = self.streaming_text.clone();
        cx.update_entity(&self.message_state, |state, cx| {
            if let Some(last) = state.messages.last_mut() {
                if last.role == MessageRole::Assistant {
                    last.content = text;
                    cx.notify();
                    return;
                }
            }
            state.messages.push(UiMessage::assistant(text));
            cx.notify();
        });
    }

    /// Surface a tool-permission request via the [`PermissionModal`] overlay.
    pub fn show_permission_modal(
        &mut self,
        req: PermissionRequest,
        cx: &mut Context<Self>,
    ) {
        if let Some(modal) = self.permission_modal.as_ref() {
            modal.update(cx, |m, cx| m.show(req, cx));
        }
    }

    /// Surface an `AskUserQuestion` prompt via the [`AskModal`] overlay. The
    /// event is stashed here (the caller has no `&mut Window`) and dispatched
    /// to `AskModal::show` during the next render via `window.defer`.
    pub fn show_ask_modal(&mut self, ev: UserQuestionEvent, cx: &mut Context<Self>) {
        self.pending_ask = Some(ev);
        cx.notify();
    }
    fn render_assistant(
        &mut self,
        ix: usize,
        item: UiMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let id: SharedString = format!("chat-{}", ix).into();
        div()
            .p_2()
            .child(TextView::markdown(id, item.content, window, cx).selectable(true))
    }

    fn render_user(
        &mut self,
        ix: usize,
        item: UiMessage,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        let id: SharedString = format!("chat-{}", ix).into();
        div()
            .p_2()
            .border_1()
            .bg(cx.theme().list_even)
            .border_color(cx.theme().border)
            .rounded_lg()
            .child(TextView::markdown(id, item.content, window, cx).selectable(true))
    }

    fn render_entry(
        &mut self,
        ix: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let items = self.message_state.read(cx).messages.clone();
        if items.len() == 0 {
            return div().into_any_element();
        }
        let item = items.get(ix).unwrap().clone();
        let elem = match item.role {
            MessageRole::ToolCall => div(),
            MessageRole::ToolResult => div(),
            MessageRole::Assistant => self.render_assistant(ix, item, window, cx),
            MessageRole::System => self.render_assistant(ix, item, window, cx),
            MessageRole::User => self.render_user(ix, item, window, cx),
        };

        div().p_1().child(elem).into_any_element()
    }

    /// Submit whatever is currently in the input box to the agent.
    ///
    /// Shared between the Send button's `on_click` and the
    /// `InputEvent::PressEnter` subscription wired up in `new()` so that
    /// pressing Enter (without modifiers) sends the message just like
    /// clicking the arrow button. Shift+Enter keeps its default
    /// line-break behaviour because the input crate maps that to
    /// `PressEnter { secondary: true }`.
    fn submit_text(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let text = self.text_input.read(cx).text().to_string();
        if text.trim().is_empty() {
            return;
        }

        // Take attached files (clears them from state)
        let files = std::mem::take(&mut self.attached_files);

        // Send chat request to agent with files
        let result = self.request_tx.try_send(AgentRequest::Chat {
            content: text.clone(),
            files,
        });

        match result {
            Ok(_) => {
                tracing::debug!("Message sent successfully");
                // Add user message to display
                self.add_message(UiMessage::user(text), cx);
                self.streaming_text.clear();
                self.last_render_at = None;
                self.set_loading(true, cx);
            }
            Err(e) => {
                tracing::error!("Failed to send message: {}", e);
                self.add_message(UiMessage::error(format!("Failed to send: {}", e)), cx);
            }
        }

        // Clear the textarea
        self.text_input.update(cx, |input, cx| {
            input.set_value("", window, cx);
        });

        cx.notify();
    }

    /// Enter-key variant of `submit_text`. Has to work without a `&mut
    /// Window` (the `cx.subscribe` callback only hands us `&mut Context`),
    /// so the read-only work runs synchronously and the input-clear step
    /// is scheduled on the next render tick via `cx.spawn` + `Entity::update`
    /// on the `text_input` entity.
    fn submit_text_from_enter(&mut self, cx: &mut Context<Self>) {
        let text = self.text_input.read(cx).text().to_string();
        if text.trim().is_empty() {
            return;
        }

        // Take attached files (clears them from state)
        let files = std::mem::take(&mut self.attached_files);

        // Send chat request to agent with files
        let result = self.request_tx.try_send(AgentRequest::Chat {
            content: text.clone(),
            files,
        });

        match result {
            Ok(_) => {
                tracing::debug!("Message sent successfully");
                // Add user message to display
                self.add_message(UiMessage::user(text), cx);
                self.streaming_text.clear();
                self.last_render_at = None;
                self.set_loading(true, cx);
            }
            Err(e) => {
                tracing::error!("Failed to send message: {}", e);
                self.add_message(UiMessage::error(format!("Failed to send: {}", e)), cx);
            }
        }

        // Clear the input on the next render tick (we can't call
        // `set_value` from here because we don't have a `&mut Window`).
        // The spawn closure hands us `cx: &mut AsyncApp`; we drop into
        // `&mut App` via the 1-arg `AsyncContext::update` trait method,
        // pick the first open window (`App::windows()`), and drive
        // `App::update_window` to acquire the `&mut Window` we need
        // for `InputState::set_value`. There is only ever one window
        // in this app, so `windows().first()` is the chat window.
        let text_input = self.text_input.clone();
        cx.spawn(async move |_this, cx| {
            let _ = cx.update(|app| {
                if let Some(window_handle) = app.windows().into_iter().next() {
                    let _ = app.update_window(window_handle, |_view, window, cx| {
                        let _ = text_input.update(cx, |input, cx| {
                            input.set_value("", window, cx);
                        });
                    });
                }
            });
        })
        .detach();

        cx.notify();
    }

    fn on_submit(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        self.submit_text(window, cx);
    }

    pub fn change_mode(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        tracing::debug!("Current mode: {:?}", cx.theme().mode);
        let new_mode = if cx.theme().mode.is_dark() {
            ThemeMode::Light
        } else {
            ThemeMode::Dark
        };
        change_color_mode(new_mode, window, cx);
    }

    pub fn clear_chat(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        self.set_loading(true, cx);
        let result = self.request_tx.try_send(AgentRequest::ClearHistory);

        match result {
            Ok(_) => {
                tracing::debug!("Chat cleared successfully");
                cx.update_entity(&self.message_state, |state, cx| {
                    state.messages.clear();
                    cx.notify();
                });
            }
            Err(e) => {
                tracing::error!("Failed to clear chat: {}", e);
                self.add_message(UiMessage::error(format!("Failed to clear chat: {}", e)), cx);
            }
        }

        self.set_loading(false, cx);
        cx.notify();
    }

    fn attachment_label(&mut self) -> String {
        match self.attached_files.clone().len() {
            0 => "Attach file".to_string(),
            1 => "1 file".to_string(),
            n => format!("{} files", n),
        }
    }

    fn on_attach_file(&mut self, _: &ClickEvent, _window: &mut Window, cx: &mut Context<Self>) {
        // Create the path prompt options - allow files, multiple selection
        let options = PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: Some("Select files to attach".into()),
        };

        // Get the receiver for the selected paths
        let paths_receiver = cx.prompt_for_paths(options);

        // Spawn an async task to handle the response
        cx.spawn(async move |this, cx| {
            // Wait for the user to select paths or cancel
            if let Ok(result) = paths_receiver.await {
                match result {
                    Ok(Some(paths)) => {
                        // User selected one or more paths
                        cx.update(|cx| {
                            let _ = this.update(cx, |chat, cx| {
                                for path in &paths {
                                    tracing::debug!("Attached file: {:?}", path);
                                }
                                chat.attached_files.extend(paths);
                                cx.notify();
                            });
                        })
                        .ok();
                    }
                    Ok(None) => {
                        // User cancelled the dialog
                        tracing::debug!("File selection cancelled");
                    }
                    Err(e) => {
                        tracing::error!("Error selecting files: {}", e);
                    }
                }
            }
        })
        .detach();
    }

    /// Open the settings panel — constructs a new `SettingsPanel` entity
    /// pre-filled with the current settings, wires its Save / Cancel
    /// callbacks (which use a `WeakEntity<ChatAI>` to mutate the view), and
    /// flips `show_settings` to true.
    pub fn open_settings(&mut self, _: &ClickEvent, window: &mut Window, cx: &mut Context<Self>) {
        // Capture a weak handle to ChatAI before entering `cx.new`, so the
        // Save/Cancel closures can mutate the view via `WeakEntity::update`.
        let weak_self = cx.weak_entity();
        let request_tx = self.request_tx.clone();
        let initial_settings = self.settings.clone();

        let panel = cx.new(|cx| {
            let mut p = SettingsPanel::new(&initial_settings, window, cx);

            // Save callback — collect the form values, persist to disk,
            // push the new config to the background agent, update the view
            // state, and close the panel.
            let request_tx = request_tx.clone();
            let weak_self_save = weak_self.clone();
            p.set_on_save(move |new_settings, _window, cx| {
                if let Err(e) = new_settings.save() {
                    tracing::error!("Failed to save settings: {}", e);
                }
                let _ = request_tx
                    .try_send(AgentRequest::SetProvider(new_settings.provider.clone()));
                let _ = request_tx.try_send(AgentRequest::SetApiConfig {
                    api_key: new_settings.api_key.clone(),
                    base_url: new_settings.effective_base_url(),
                });
                let _ = request_tx.try_send(AgentRequest::SetModel(new_settings.model.clone()));
                let _ = request_tx
                    .try_send(AgentRequest::SetWorkingDir(new_settings.working_dir.clone()));

                // Reflect the new settings back onto the ChatAI view:
                // update `self.settings`, refresh `has_api_key`, and hide
                // the panel.
                let _ = weak_self_save.update(cx, |view, cx| {
                    view.has_api_key = new_settings.has_api_key();
                    view.settings = new_settings.clone();
                    view.show_settings = false;
                    view.settings_panel = None;
                    cx.notify();
                });
            });

            // Cancel callback — just hide the panel.
            let weak_self_cancel = weak_self.clone();
            p.set_on_cancel(move |_window, cx| {
                let _ = weak_self_cancel.update(cx, |view, cx| {
                    view.show_settings = false;
                    view.settings_panel = None;
                    cx.notify();
                });
            });

            p
        });

        self.settings_panel = Some(panel);
        self.show_settings = true;
        cx.notify();
    }
}

impl Render for ChatAI {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // Drain a pending AskUserQuestion: `show_ask_modal` stashes the event
        // (it has no `&mut Window`), so we dispatch it to `AskModal::show`
        // here, deferred to after this render pass to avoid mutating during
        // render.
        if let Some(ev) = self.pending_ask.take() {
            let modal = self.ask_modal.clone();
            _window.defer(cx, move |window, cx| {
                if let Some(m) = &modal {
                    let _ = m.update(cx, |m, ctx| m.show(ev, window, ctx));
                }
            });
        }

        let items_len = self.message_state.read(cx).messages.clone().len();

        let theme_toggle = Button::new("theme-mode")
            .map(|this| {
                if cx.theme().mode.is_dark() {
                    this.icon(Icon::empty().path("icons/sun.svg"))
                } else {
                    this.icon(Icon::empty().path("icons/moon.svg"))
                }
            })
            .small()
            .tooltip("Change mode")
            .ghost()
            .on_click(cx.listener(Self::change_mode));

        let clear_chat = Button::new("clear-chat")
            .icon(Icon::empty().path("icons/square-pen.svg"))
            .tooltip("New chat")
            .small()
            .ghost()
            .on_click(cx.listener(Self::clear_chat));

        let settings_btn = Button::new("settings")
            .icon(Icon::empty().path("icons/settings.svg"))
            .tooltip("Settings")
            .small()
            .ghost()
            .on_click(cx.listener(Self::open_settings));

        let header = TitleBar::new().child(
            h_flex()
                .w_full()
                .py_1()
                .pr_1()
                .justify_between()
                .child(Label::new("LFAgent"))
                .child(
                    div()
                        .pr(px(5.0))
                        .flex()
                        .items_center()
                        .gap_1()
                        .when(items_len > 0, |d| d.child(clear_chat))
                        .child(settings_btn)
                        .child(theme_toggle),
                ),
        );

        let empty_content = div()
            .flex()
            .flex_col()
            .flex_grow()
            .justify_end()
            .gap_4()
            .p_4()
            .when(!self.has_api_key.clone(), |d| {
                d.child(Alert::error("no-api-key", "No Anthropic API Key Found"))
            })
            .child(
                div()
                    .flex()
                    .w_full()
                    .gap_2()
                    .justify_start()
                    .items_center()
                    .child(Icon::empty().path("icons/pencil-line.svg"))
                    .child(Label::new("Draft a reply")),
            )
            .child(
                div()
                    .flex()
                    .w_full()
                    .gap_2()
                    .justify_start()
                    .items_center()
                    .child(Icon::empty().path("icons/wand-sparkles.svg"))
                    .child(Label::new("Summarize an email")),
            );

        let form_header = div()
            .flex()
            .gap_1()
            .p_2()
            .justify_start()
            .items_center()
            .child(
                Button::new("add-file")
                    .icon(Icon::empty().path("icons/paperclip.svg"))
                    .ghost()
                    .mr_1()
                    .on_click(cx.listener(Self::on_attach_file)),
            )
            .child(Divider::vertical())
            .child(Label::new(self.attachment_label()).pl_2());

        let form_footer = div()
            .flex()
            .gap_2()
            .p_2()
            .justify_between()
            .items_center()
            .child(
                div()
                    .flex()
                    .justify_start()
                    .gap_1()
                    .pl_2()
                    .items_center()
                    .child(Icon::empty().path("icons/anthropic.svg"))
                    .child(Label::new(self.settings.model.clone())),
            )
            .child(if self.is_loading {
                Button::new("stop")
                    .rounded_full()
                    .danger()
                    .label("Stop")
                    .on_click(cx.listener(|this, _, _, _cx| {
                        let _ = this.request_tx.try_send(AgentRequest::Cancel);
                    }))
            } else {
                Button::new("send")
                    .rounded_full()
                    .bg(cx.theme().accent)
                    .icon(Icon::empty().path("icons/move-up.svg"))
                    .on_click(cx.listener(Self::on_submit))
            });

        let form = div()
            .flex()
            .flex_col()
            .justify_between()
            .rounded_2xl()
            .border_1()
            .border_color(cx.theme().border.opacity(0.8))
            .bg(cx.theme().popover)
            .h(px(180.))
            .shadow_lg()
            .w_full()
            .child(
                div().flex().flex_col().child(form_header).child(
                    Input::new(&self.text_input.clone())
                        .appearance(false)
                        .disabled(!self.has_api_key.clone()),
                ),
            )
            .child(form_footer);

        let main = div().v_flex().size_full().child(header).child(
            div()
                .p_2()
                .v_flex()
                .size_full()
                .when(items_len == 0, |d| d.child(empty_content))
                .when(items_len > 0, |d| {
                    d.child(
                        div().p_2().size_full().flex().child(
                            list(
                                self.list_state.clone(),
                                cx.processor(|this, ix, window, cx| {
                                    this.render_entry(ix, window, cx)
                                }),
                            )
                            .size_full(),
                        ),
                    )
                })
                .child(form),
        );

        // Wrap everything in a relative container so the overlay modals
        // (settings panel, permission modal, ask modal) can absolute-position
        // themselves over the chat.
        let mut root = div().size_full().relative().child(main);

        if self.show_settings {
            if let Some(panel) = &self.settings_panel {
                root = root.child(panel.clone());
            }
        }
        if let Some(pm) = &self.permission_modal {
            root = root.child(pm.clone());
        }
        if let Some(am) = &self.ask_modal {
            root = root.child(am.clone());
        }

        root.into_any_element()
    }
}
