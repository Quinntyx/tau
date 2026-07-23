# Idiomatic GPUI Architecture & Project Design Blueprint

This document specifies the architectural principles, crate/module layouts, design patterns, component paradigms, and state management techniques for building production-grade, idiomatic GPUI applications in Rust, modeled after the software architecture of [Zed](https://github.com/zed-industries/zed).

---

## Table of Contents
1. [Core Architectural Philosophy](#1-core-architectural-philosophy)
2. [Crate & Project Layout (Zed-Shaped Architecture)](#2-crate--project-layout-zed-shaped-architecture)
3. [GPUI Primitive Concepts & Mental Model](#3-gpui-primitive-concepts--mental-model)
4. [Component Design System (`crates/ui`)](#4-component-design-system-cratesui)
5. [State Management & Data Flow Patterns](#5-state-management--data-flow-patterns)
6. [View Composition & Panel Management](#6-view-composition--panel-management)
7. [Theme, Colors, Typography, & Spacing](#7-theme-colors-typography--spacing)
8. [Overlays, Popovers, & Layering Strategies](#8-overlays-popovers--layering-strategies)
9. [Event Handling, Focus, Keyboard Navigation, & Action System](#9-event-handling-focus-keyboard-navigation--action-system)
10. [Virtualized Lists & Performance Optimization](#10-virtualized-lists--performance-optimization)
11. [Low-Level Custom Elements (`Element`, `prepaint`, `paint`)](#11-low-level-custom-elements-element-prepaint-paint)
12. [Asynchronous I/O & Model Syncing](#12-asynchronous-io--model-syncing)
13. [Testing Patterns for GPUI Applications](#13-testing-patterns-for-gpui-applications)
14. [Complete Example: Zed-Shaped Todo Application](#14-complete-example-zed-shaped-todo-application)

---

## 1. Core Architectural Philosophy

A well-structured GPUI codebase adheres to the following core tenets:

1. **Strict Separation of Model, View, and Element**:
   - **Model (`Entity<T>`)**: Pure application state and domain logic, decoupled from any UI representation.
   - **View (`Render`)**: Long-lived, reactive UI controllers (wrapped in `Entity<V>`) that manage UI state, listen to model change notifications, and assemble elements.
   - **Element (`RenderOnce` / `Element`)**: Short-lived, ephemeral layout nodes constructed on demand during frame rendering.

2. **Composition over Monolithic Views**:
   - Never build a single monolithic `view.rs` with 3,000 lines. Break UIs down into specialized, modular views (e.g., `Header`, `Sidebar`, `Inspector`, `Modal`, `Picker`, `Composer`, `Feed`).

3. **Fluent Builder Pattern for Components**:
   - Stateless UI widgets (Buttons, Inputs, Cards, Badges, Labels) implement `RenderOnce` and `#[derive(IntoElement)]`. They expose chainable, builder-style builder methods (`.style()`, `.on_click()`, `.disabled()`, `.icon()`).

4. **Centralized Action System & Keymaps**:
   - Keyboard shortcuts, menu commands, and user triggers dispatch strongly-typed `gpui::Action` structs using `cx.dispatch_action(...)` or `.on_action(...)`, separating keybindings from execution handlers.

5. **Theme Tokenization**:
   - Hardcoded hex/RGB values (`0x33445f`) are strictly forbidden in UI rendering. All colors, borders, font weights, and spacing must pull from semantic theme tokens (`cx.theme().surface`, `cx.theme().text`, `cx.theme().border`).

---

## 2. Crate & Project Layout (Zed-Shaped Architecture)

A Zed-shaped workspace cleanly isolates domain models, shared UI design primitives, application services, and view modules.

```
my-app/
├── Cargo.toml
├── FRONTEND.md
├── crates/
│   ├── app/                      # Main application entry point & window bootstrapping
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── main.rs
│   │       └── window.rs
│   ├── core/                     # Pure domain logic, storage, & data structures (No GPUI dependency)
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── todo.rs
│   │       └── store.rs
│   ├── ui/                       # Reusable UI component library & design system
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── prelude.rs
│   │       ├── traits.rs
│   │       ├── components/
│   │       │   ├── button.rs
│   │       │   ├── card.rs
│   │       │   ├── icon.rs
│   │       │   ├── label.rs
│   │       │   ├── input.rs
│   │       │   ├── modal.rs
│   │       │   └── popover.rs
│   │       └── styles/
│   │           ├── colors.rs
│   │           ├── typography.rs
│   │           └── spacing.rs
│   ├── theme/                    # Theme registry, active theme provider, and color tokens
│   │   ├── Cargo.toml
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── active_theme.rs
│   │       └── schema.rs
│   └── workspace/                # Top-level application view controllers & panel navigation
│       ├── Cargo.toml
│       └── src/
│           ├── lib.rs
│           ├── workspace.rs
│           ├── header.rs
│           ├── sidebar.rs
│           ├── inspector.rs
│           └── picker.rs
```

---

## 3. GPUI Primitive Concepts & Mental Model

### `Entity<T>` & `Model<T>`
An `Entity<T>` (or `Model<T>`) is a thread-safe, reference-counted handle (`Arc`-like) to state owned by GPUI's `App`.
- Mutate state via `entity.update(cx, |model, cx| { ... })`.
- Notify listeners via `cx.notify()`.
- Observe entity changes via `cx.observe(&entity, |this, entity, cx| { ... })`.

### `View<V>`
A `View` is an `Entity` whose inner type `V` implements `gpui::Render`. Views manage long-lived UI sections, capture focus, and listen to model events.

### `RenderOnce` vs `Render`
| Feature | `RenderOnce` | `Render` |
|---|---|---|
| **Lifetime** | Instantiated per frame, discarded after paint | Long-lived, stored in `Entity<V>` |
| **State** | Purely visual / configuration props | Holds state, focus handles, async tasks |
| **Use Case** | Buttons, Cards, Labels, Modals, Badges | Workspace, Editor, Sidebar, Picker |

---

## 4. Component Design System (`crates/ui`)

Every UI component in `crates/ui` follows this idiomatic implementation pattern:

### 1. Component Struct & `#[derive(IntoElement)]`
```rust
use gpui::*;

#[derive(IntoElement)]
pub struct Button {
    id: ElementId,
    label: SharedString,
    icon: Option<SharedString>,
    disabled: bool,
    on_click: Option<Box<dyn Fn(&ClickEvent, &mut Window, &mut App) + 'static>>,
}

impl Button {
    pub fn new(id: impl Into<ElementId>, label: impl Into<SharedString>) -> Self {
        Self {
            id: id.into(),
            label: label.into(),
            icon: None,
            disabled: false,
            on_click: None,
        }
    }

    pub fn icon(mut self, icon: impl Into<SharedString>) -> Self {
        self.icon = Some(icon.into());
        self
    }

    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn on_click(mut self, handler: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> Self {
        self.on_click = Some(Box::new(handler));
        self
    }
}

impl RenderOnce for Button {
    fn render(self, _window: &mut Window, cx: &mut App) -> impl IntoElement {
        let theme = cx.global::<ActiveTheme>();

        div()
            .id(self.id)
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .py_1p5()
            .rounded_md()
            .bg(theme.colors.surface)
            .text_color(theme.colors.text)
            .border_1()
            .border_color(theme.colors.border)
            .hover(|style| style.bg(theme.colors.surface_hover))
            .when(self.disabled, |this| this.opacity(0.5).cursor_not_allowed())
            .when_some(self.on_click, |this, handler| {
                this.cursor_pointer().on_click(handler)
            })
            .when_some(self.icon, |this, icon| {
                this.child(div().child(icon))
            })
            .child(self.label)
    }
}
```

---

## 5. State Management & Data Flow Patterns

### Event Subscription & Observation Pattern
```rust
pub struct TaskListController {
    store: Entity<TaskStore>,
}

impl TaskListController {
    pub fn new(store: Entity<TaskStore>, cx: &mut Context<Self>) -> Self {
        // Automatically re-render TaskListController whenever TaskStore notifies
        cx.observe(&store, |this, _store, cx| {
            cx.notify();
        }).detach();

        Self { store }
    }
}
```

---

## 6. View Composition & Panel Management

In Zed-shaped layout design, the root view delegates layout to distinct child views:

```rust
impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .flex()
            .flex_row()
            .bg(cx.theme().canvas)
            // Left Panel (Sidebar)
            .when(self.sidebar_open, |this| {
                this.child(self.sidebar.clone())
            })
            // Main Center Column
            .child(
                div()
                    .flex_1()
                    .flex_col()
                    .min_w_0()
                    .child(self.header.clone())
                    .child(div().flex_1().child(self.content.clone()))
                    .child(self.composer.clone())
            )
            // Right Panel (Inspector)
            .when(self.inspector_open, |this| {
                this.child(self.inspector.clone())
            })
    }
}
```

---

## 7. Theme, Colors, Typography, & Spacing

### Centralized Active Theme Trait
```rust
pub struct ThemeColors {
    pub canvas: Rgba,
    pub sidebar: Rgba,
    pub surface: Rgba,
    pub surface_hover: Rgba,
    pub border: Rgba,
    pub text: Rgba,
    pub text_muted: Rgba,
    pub accent: Rgba,
    pub error: Rgba,
}

pub struct ActiveTheme {
    pub colors: ThemeColors,
}

pub trait ThemeExt {
    fn theme(&self) -> &ThemeColors;
}

impl ThemeExt for App {
    fn theme(&self) -> &ThemeColors {
        &self.global::<ActiveTheme>().colors
    }
}
```

---

## 8. Overlays, Popovers, & Layering Strategies

To prevent popovers, modals, and tooltips from clipping inside parent views, wrap overlay elements in `gpui::deferred`:

```rust
if self.picker_open {
    root = root.child(
        deferred(
            div()
                .absolute()
                .top_12()
                .left_half()
                .child(self.picker.clone())
        )
        .with_priority(10) // Higher priority paints above lower priority
    );
}
```

---

## 9. Event Handling, Focus, Keyboard Navigation, & Action System

### 1. Defining Actions
Actions are strongly-typed identifier structs registered with GPUI:
```rust
gpui::actions!(todo, [CreateTask, DeleteTask, ToggleTask, FilterAll, FilterActive, FilterCompleted]);
```

### 2. Focus Handles & Key Contexts
Focus handles allow views to receive keyboard events. Every focusable view retains a `FocusHandle`:
```rust
pub struct TaskInput {
    focus_handle: FocusHandle,
}

impl Focusable for TaskInput {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}
```

Bind key contexts and actions to render trees:
```rust
div()
    .key_context("TodoList")
    .track_focus(&self.focus_handle)
    .on_action(cx.listener(Self::handle_create_task))
    .on_action(cx.listener(Self::handle_toggle_task))
```

---

## 10. Virtualized Lists & Performance Optimization

For long lists (thousands of items), rendering standard `div()` elements causes performance issues. GPUI provides `uniform_list`:

```rust
uniform_list(
    cx.view().clone(),
    "todo-list",
    items.len(),
    move |this, range, _window, cx| {
        range.map(|ix| {
            let item = &this.items[ix];
            div().child(item.title.clone())
        }).collect()
    }
)
.track_scroll(&self.scroll_handle)
```

---

## 11. Low-Level Custom Elements (`Element`, `prepaint`, `paint`)

For specialized components requiring custom canvas layout or text shaping (such as code editors or custom graphs), implement `gpui::Element`:

```rust
pub struct CustomCanvasElement {
    color: Rgba,
}

impl Element for CustomCanvasElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> { None }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = px(200.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _window: &mut Window,
        _cx: &mut App,
    ) -> Self::PrepaintState {
        ()
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut Self::RequestLayoutState,
        _: &mut Self::PrepaintState,
        window: &mut Window,
        _cx: &mut App,
    ) {
        window.paint_quad(fill(bounds, self.color));
    }
}
```

---

## 12. Asynchronous I/O & Model Syncing

Use `cx.spawn()` to perform async operations safely without blocking UI rendering:

```rust
pub fn fetch_items(&mut self, cx: &mut Context<Self>) {
    self.loading = true;
    cx.notify();

    cx.spawn(|this, mut cx| async move {
        let items = fetch_from_api().await;
        this.update(&mut cx, |view, cx| {
            view.items = items;
            view.loading = false;
            cx.notify();
        }).ok();
    }).detach();
}
```

---

## 13. Testing Patterns for GPUI Applications

### Unit & Headless View Testing
```rust
#[gpui::test]
async fn test_task_creation(cx: &mut TestAppContext) {
    let store = cx.new_model(|_| TaskStore::new());
    let view = cx.add_window(|cx| TaskListController::new(store.clone(), cx));

    view.update(cx, |controller, cx| {
        controller.add_task("Buy groceries", cx);
    });

    store.read_with(cx, |store, _| {
        assert_eq!(store.tasks.len(), 1);
        assert_eq!(store.tasks[0].title, "Buy groceries");
    });
}
```

---

## 14. Complete Example: Zed-Shaped Todo Application

Below is a complete reference implementation of a Zed-shaped Todo Application demonstrating all architectural principles described in this document.

### `crates/core/src/todo.rs`
```rust
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TodoItem {
    pub id: u64,
    pub title: String,
    pub completed: bool,
}

pub struct TodoStore {
    next_id: u64,
    items: Vec<TodoItem>,
}

impl TodoStore {
    pub fn new() -> Self {
        Self {
            next_id: 1,
            items: Vec::new(),
        }
    }

    pub fn add(&mut self, title: impl Into<String>) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        self.items.push(TodoItem {
            id,
            title: title.into(),
            completed: false,
        });
        id
    }

    pub fn toggle(&mut self, id: u64) {
        if let Some(item) = self.items.iter_mut().find(|i| i.id == id) {
            item.completed = !item.completed;
        }
    }

    pub fn items(&self) -> &[TodoItem] {
        &self.items
    }
}
```

### `crates/workspace/src/workspace.rs`
```rust
use gpui::*;
use ui::prelude::*;
use core::todo::TodoStore;

pub struct Workspace {
    store: Entity<TodoStore>,
}

impl Workspace {
    pub fn new(store: Entity<TodoStore>, cx: &mut Context<Self>) -> Self {
        cx.observe(&store, |_this, _store, cx| {
            cx.notify();
        }).detach();

        Self { store }
    }
}

impl Render for Workspace {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let items = self.store.read(cx).items().to_vec();

        v_flex()
            .size_full()
            .bg(cx.theme().canvas)
            .p_4()
            .gap_3()
            .child(
                Label::new("Todo Manager")
                    .size(HeadlineSize::Large)
            )
            .children(items.into_iter().map(|item| {
                let id = item.id;
                let store = self.store.clone();
                h_flex()
                    .key_context("TodoItem")
                    .justify_between()
                    .p_2()
                    .rounded_md()
                    .bg(cx.theme().surface)
                    .child(Label::new(item.title))
                    .child(
                        Button::new(format!("toggle-{id}"), if item.completed { "Done" } else { "Todo" })
                            .on_click(move |_, _, cx| {
                                store.update(cx, |store, cx| {
                                    store.toggle(id);
                                    cx.notify();
                                });
                            })
                    )
            }))
    }
}
```
