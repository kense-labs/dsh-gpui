//! Native task catalog backed by the Harness Schedule service.

use super::*;
use gpui::{Div, Entity, Stateful};
use serde::Deserialize;

const SCHEDULE_CHAT_PROMPT: &str = "请根据需求创建一个自动化任务。";

#[derive(Clone, Copy, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum Status {
    #[default]
    Active,
    Paused,
    Ended,
}

impl Status {
    fn label(self) -> &'static str {
        match self {
            Self::Active => "已开启",
            Self::Paused => "已暂停",
            Self::Ended => "已结束",
        }
    }
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Task {
    id: String,
    kind: String,
    title: String,
    prompt: String,
    status: Status,
    scheduled_at: String,
    updated_at: Option<String>,
    target: Target,
    #[serde(default)]
    execution_active: bool,
    delivery_floor_ends_at: Option<String>,
    #[serde(flatten)]
    timing: Value,
}

impl Task {
    fn frequency(&self) -> String {
        match self.kind.as_str() {
            "after" => format!(
                "延迟 {}",
                seconds(self.timing["afterSeconds"].as_u64().unwrap_or(0))
            ),
            "every" => format!(
                "每隔 {}",
                seconds(self.timing["everySeconds"].as_u64().unwrap_or(0))
            ),
            "daily" => format!(
                "每天 {}",
                clock_text(self.timing["time"].as_str().unwrap_or(""))
            ),
            "weekly" => format!(
                "每周 {} {}",
                weekday_label(&self.timing["weekdays"]),
                clock_text(self.timing["time"].as_str().unwrap_or(""))
            ),
            "cron" => self.timing["expression"]
                .as_str()
                .unwrap_or("Cron")
                .to_owned(),
            "rrule" => "RRULE".to_owned(),
            _ => "单次运行".to_owned(),
        }
    }

    fn edit(&self, action: &str, patch: Option<Value>) -> Value {
        let mut request =
            json!({"id": self.id, "action": action, "expectedUpdatedAt": self.updated_at});
        if let Some(patch) = patch {
            request["patch"] = patch;
        }
        json!({"action":"edit", "request":request})
    }
}

#[derive(Clone, Deserialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
enum Target {
    Session { session_id: String },
    NewOnce { session_id: String },
    New,
}

impl Target {
    fn session_id(&self) -> Option<&str> {
        match self {
            Self::Session { session_id } | Self::NewOnce { session_id } => Some(session_id),
            Self::New => None,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Catalog {
    available: bool,
    #[serde(default)]
    can_create_session: bool,
    #[serde(default)]
    conversation_tools: bool,
    #[serde(default)]
    tasks: Vec<Task>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HistoryPage {
    records: Vec<Delivery>,
    next_before: Option<String>,
    earlier_records_pruned: bool,
    earlier_records_unavailable: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Delivery {
    delivered_at: String,
    scheduled_at: String,
    target_session_id: String,
    prompt: Option<String>,
}

#[derive(Default)]
struct History {
    page: Option<HistoryPage>,
    request: Option<Rc<()>>,
    error: Option<String>,
}

enum Confirmation {
    Delete(Task),
    ClearEnded(usize),
}

struct CreateMenu {
    trigger: gpui::FocusHandle,
    items: [gpui::FocusHandle; 2],
    open: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum RuleKind {
    After,
    At,
    Every,
    Daily,
    Weekly,
    Cron,
    RRule,
}

impl RuleKind {
    const ALL: [Self; 7] = [
        Self::After,
        Self::At,
        Self::Every,
        Self::Daily,
        Self::Weekly,
        Self::Cron,
        Self::RRule,
    ];

    fn from_task(task: &Task) -> Self {
        match task.kind.as_str() {
            "at" => Self::At,
            "every" => Self::Every,
            "daily" => Self::Daily,
            "weekly" => Self::Weekly,
            "cron" => Self::Cron,
            "rrule" => Self::RRule,
            _ => Self::After,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::After => "延迟",
            Self::At => "指定时间",
            Self::Every => "间隔",
            Self::Daily => "每天",
            Self::Weekly => "每周",
            Self::Cron => "Cron",
            Self::RRule => "RRULE",
        }
    }

    fn field(self) -> (&'static str, &'static str) {
        match self {
            Self::After => ("延迟秒数", "例如 600"),
            Self::At => ("运行时间", "YYYY-MM-DD HH:mm:ss"),
            Self::Every => ("间隔秒数", "例如 3600"),
            Self::Daily | Self::Weekly => ("运行时间", "例如 09:00:00"),
            Self::Cron => ("Cron 表达式", "例如 0 9 * * 1-5"),
            Self::RRule => ("重复规则", "DTSTART 和 RRULE 各一行"),
        }
    }

    fn needs_zone(self) -> bool {
        matches!(self, Self::At | Self::Daily | Self::Weekly | Self::Cron)
    }

    fn original(self, task: &Task) -> String {
        match self {
            Self::After => task.timing["afterSeconds"].to_string(),
            Self::Every => task.timing["everySeconds"].to_string(),
            Self::At => task.scheduled_at.clone(),
            Self::Daily | Self::Weekly => task.timing["time"].as_str().unwrap_or("").to_owned(),
            Self::Cron => task.timing["expression"].as_str().unwrap_or("").to_owned(),
            Self::RRule => task.timing["rrule"].as_str().unwrap_or("").to_owned(),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Destination {
    Session,
    NewOnce,
    New,
}

impl Destination {
    fn title(self) -> &'static str {
        match self {
            Self::Session => "已有会话",
            Self::NewOnce => "创建并复用会话",
            Self::New => "每次新建会话",
        }
    }
}

struct Draft {
    original: Option<Task>,
    return_focus: Option<gpui::FocusHandle>,
    name: Entity<Editor>,
    prompt: Entity<Editor>,
    rule: RuleKind,
    value: Entity<Editor>,
    zone: Entity<Editor>,
    weekdays: Vec<u8>,
    destination: Destination,
    session: Option<String>,
    choose_session: bool,
    _changes: Vec<Subscription>,
}

impl Draft {
    fn timing(&self, cx: &App) -> Result<Value, &'static str> {
        let value = self.value.read(cx).value().trim();
        let time_zone = self.zone.read(cx).value().trim();
        if value.is_empty() || (self.rule.needs_zone() && time_zone.is_empty()) {
            return Err("请填写任务的运行时间和时区。");
        }
        let interval = || {
            value
                .parse::<u64>()
                .ok()
                .filter(|value| (1..=9_007_199_254_740_991).contains(value))
                .ok_or("请输入大于零的整数秒数。")
        };
        Ok(match self.rule {
            RuleKind::After => json!({"kind":"after","after_seconds":interval()?}),
            RuleKind::At => {
                let at = if let Some((date, time)) = value.split_once(' ') {
                    json!({"date":date,"time":time,"time_zone":time_zone})
                } else {
                    json!(value)
                };
                json!({"kind":"at","at":at})
            }
            RuleKind::Every => json!({"kind":"every","every_seconds":interval()?}),
            RuleKind::Daily => json!({"kind":"daily","daily":{"time":value,"time_zone":time_zone}}),
            RuleKind::Weekly if self.weekdays.is_empty() => return Err("请选择至少一个星期。"),
            RuleKind::Weekly => {
                json!({"kind":"weekly","weekly":{"time":value,"time_zone":time_zone,"weekdays":self.weekdays}})
            }
            RuleKind::Cron => {
                json!({"kind":"cron","cron":{"expression":value,"time_zone":time_zone}})
            }
            RuleKind::RRule => json!({"kind":"rrule","rrule":value}),
        })
    }

    fn request(&self, cwd: &str, cx: &App) -> Result<Value, &'static str> {
        let name = self.name.read(cx).value().trim();
        let prompt = self.prompt.read(cx).value().trim();
        if name.is_empty() || prompt.is_empty() {
            return Err("请填写任务名称和指令。");
        }
        let target = match self.destination {
            Destination::Session => {
                json!({"kind":"session","sessionId":self.session.as_ref().ok_or("请选择投递会话。")?})
            }
            destination => {
                json!({"kind":if destination == Destination::New {"new"} else {"new_once"},"create":{"cwd":cwd}})
            }
        };
        let Some(task) = &self.original else {
            return Ok(
                json!({"action":"create","request":{"title":name,"prompt":prompt,"timing":self.timing(cx)?,"target":target}}),
            );
        };
        let mut patch = serde_json::Map::new();
        if name != task.title {
            patch.insert("title".into(), json!(name));
        }
        if prompt != task.prompt {
            patch.insert("prompt".into(), json!(prompt));
        }
        let original_rule = RuleKind::from_task(task);
        let zone_changed = self.rule.needs_zone()
            && self.zone.read(cx).value().trim()
                != task.timing["timeZone"].as_str().unwrap_or("UTC");
        if self.rule != original_rule
            || self.value.read(cx).value().trim() != original_rule.original(task)
            || zone_changed
            || (self.rule == RuleKind::Weekly && json!(self.weekdays) != task.timing["weekdays"])
        {
            patch.insert("timing".into(), self.timing(cx)?);
        }
        let target_changed = match (&task.target, self.destination) {
            (Target::Session { session_id }, Destination::Session) => {
                self.session.as_ref() != Some(session_id)
            }
            (Target::NewOnce { .. }, Destination::NewOnce) | (Target::New, Destination::New) => {
                false
            }
            _ => true,
        };
        if target_changed {
            patch.insert("target".into(), target);
        }
        if patch.is_empty() {
            return Err("没有需要保存的变更。");
        }
        Ok(task.edit("update", Some(Value::Object(patch))))
    }
}

#[derive(Default)]
pub(super) struct Automations {
    catalog: Option<Catalog>,
    read: Option<Rc<()>>,
    requested: bool,
    refresh: bool,
    error: Option<String>,
    mutation: Option<Rc<()>>,
    mutation_error: Option<String>,
    filter: Option<Status>,
    search: Option<(Entity<Editor>, Subscription)>,
    detail: Option<String>,
    show_history: bool,
    history: History,
    confirm: Option<Confirmation>,
    confirm_focus: Option<gpui::FocusHandle>,
    floor_refresh: Option<String>,
    draft: Option<Draft>,
    create_menu: Option<CreateMenu>,
}

impl Automations {
    pub(super) fn new() -> Self {
        Self {
            filter: Some(Status::Active),
            ..Self::default()
        }
    }
}

impl DshApp {
    fn start_automation_chat(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        self.close_automation_create_menu();
        self.close_automation_detail(w, cx);
        self.new_session_at(self.workspace.clone(), Some(SCHEDULE_CHAT_PROMPT), w, cx);
    }

    pub(super) fn close_automation_create_menu(&mut self) {
        if let Some(menu) = &mut self.automations.create_menu {
            menu.open = false;
        }
    }

    fn toggle_automation_create_menu(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        let Some(menu) = &mut self.automations.create_menu else {
            return;
        };
        menu.open = !menu.open;
        if menu.open {
            menu.items[0].focus(w, cx);
        } else {
            menu.trigger.focus(w, cx);
        }
        cx.notify();
    }

    fn open_automation_editor(
        &mut self,
        original: Option<Task>,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.automations.mutation.is_some() {
            return;
        }
        let return_focus = self
            .automations
            .create_menu
            .as_ref()
            .filter(|menu| menu.open)
            .map(|menu| menu.trigger.clone());
        self.close_automation_create_menu();
        let can_create_session = self
            .automations
            .catalog
            .as_ref()
            .is_some_and(|catalog| catalog.can_create_session);
        let rule = original
            .as_ref()
            .map_or(RuleKind::Daily, RuleKind::from_task);
        let (name, n) = automation_field(
            "任务名称",
            InputMode::Single,
            original.as_ref().map_or("", |task| &task.title),
            w,
            cx,
        );
        let (prompt, p) = automation_field(
            "描述任务要执行的内容",
            InputMode::Multi,
            original.as_ref().map_or("", |task| &task.prompt),
            w,
            cx,
        );
        let value_text = original
            .as_ref()
            .map_or_else(|| "09:00:00".into(), |task| rule.original(task));
        let (value, v) = automation_field(rule.field().1, InputMode::Multi, &value_text, w, cx);
        let locale_zone = local_zone();
        let zone_text = original
            .as_ref()
            .and_then(|task| task.timing["timeZone"].as_str())
            .unwrap_or(if original.is_some() {
                "UTC"
            } else {
                &locale_zone
            });
        let (zone, z) = automation_field("例如 Asia/Shanghai", InputMode::Single, zone_text, w, cx);
        let weekdays = original
            .as_ref()
            .and_then(|task| serde_json::from_value(task.timing["weekdays"].clone()).ok())
            .unwrap_or_else(|| vec![1]);
        let destination = match original.as_ref().map(|task| &task.target) {
            Some(Target::Session { .. }) => Destination::Session,
            Some(Target::NewOnce { .. }) => Destination::NewOnce,
            Some(Target::New) => Destination::New,
            None if can_create_session => Destination::NewOnce,
            None => Destination::Session,
        };
        let session = original
            .as_ref()
            .and_then(|task| task.target.session_id())
            .map(str::to_owned)
            .or_else(|| {
                self.active_session
                    .clone()
                    .filter(|id| !self.archived_sessions.contains(id))
            })
            .or_else(|| {
                self.sessions
                    .iter()
                    .find(|session| !self.archived_sessions.contains(&session.id))
                    .map(|session| session.id.clone())
            });
        name.update(cx, |input, cx| input.focus(w, cx));
        self.automations.draft = Some(Draft {
            original,
            return_focus,
            name,
            prompt,
            rule,
            value,
            zone,
            weekdays,
            destination,
            session,
            choose_session: false,
            _changes: vec![n, p, v, z],
        });
        self.automations.mutation_error = None;
        cx.notify();
    }

    fn save_automation_editor(&mut self, cx: &mut Context<Self>) {
        let Some(draft) = self.automations.draft.as_ref() else {
            return;
        };
        match draft.request(&self.workspace, cx) {
            Ok(operation) => self.schedule_mutation(operation, cx),
            Err(error) => {
                self.automations.mutation_error = Some(error.into());
                cx.notify();
            }
        }
    }

    fn automation_editor(&self, draft: &Draft, cx: &mut Context<Self>) -> Div {
        let busy = self.automations.mutation.is_some();
        let can_create_session = self
            .automations
            .catalog
            .as_ref()
            .is_some_and(|catalog| catalog.can_create_session);
        let mut choices = row().w_full().gap(px(4.)).flex_wrap();
        for rule in RuleKind::ALL {
            choices = choices.child(chip(
                rule.name(),
                rule.name(),
                draft.rule == rule,
                !busy,
                cx,
                move |app, w, cx| {
                    if let Some(draft) = &mut app.automations.draft
                        && draft.rule != rule
                    {
                        draft.rule = rule;
                        let field = rule.field().1;
                        draft
                            .value
                            .update(cx, |editor, _| editor.set_placeholder(field));
                        replace(&draft.value, "", w, cx);
                        cx.notify();
                    }
                },
            ));
        }
        let mut destination = row().w_full().gap(px(4.)).flex_wrap();
        for target in [Destination::Session, Destination::NewOnce, Destination::New] {
            destination = destination.child(chip(
                target.title(),
                target.title(),
                draft.destination == target,
                !busy && (target == Destination::Session || can_create_session),
                cx,
                move |app, _, cx| {
                    if let Some(draft) = &mut app.automations.draft {
                        draft.destination = target;
                        draft.choose_session = false;
                        cx.notify();
                    }
                },
            ));
        }
        let mut body = column()
            .w_full()
            .gap(px(12.))
            .child(detail_header(
                if draft.original.is_some() {
                    "编辑任务"
                } else {
                    "新建任务"
                },
                !busy,
                cx,
            ))
            .child(label("任务名称", cx))
            .child(schedule_field(&draft.name, 36., cx))
            .child(label("运行频率", cx))
            .child(choices)
            .child(label(draft.rule.field().0, cx))
            .child(schedule_field(
                &draft.value,
                if draft.rule == RuleKind::RRule {
                    74.
                } else {
                    36.
                },
                cx,
            ))
            .when(draft.rule.needs_zone(), |body| {
                body.child(label("时区", cx))
                    .child(schedule_field(&draft.zone, 36., cx))
            });
        if draft.rule == RuleKind::Weekly {
            let mut days = row().w_full().gap(px(4.)).flex_wrap();
            for (day, text) in ["一", "二", "三", "四", "五", "六", "日"]
                .into_iter()
                .enumerate()
            {
                let day = day as u8 + 1;
                days = days.child(chip(
                    text,
                    text,
                    draft.weekdays.contains(&day),
                    !busy,
                    cx,
                    move |app, _, cx| {
                        if let Some(draft) = &mut app.automations.draft {
                            if draft.weekdays.contains(&day) {
                                draft.weekdays.retain(|value| *value != day);
                            } else {
                                draft.weekdays.push(day);
                                draft.weekdays.sort_unstable();
                            }
                            cx.notify();
                        }
                    },
                ));
            }
            body = body.child(days);
        }
        body = body.child(label("投递目标", cx)).child(destination);
        if draft.destination == Destination::Session {
            let selected = draft
                .session
                .as_ref()
                .and_then(|id| self.sessions.iter().find(|session| &session.id == id));
            body = body.child(action(
                "schedule-target-select",
                selected
                    .map_or("选择会话", SessionSummary::title)
                    .to_owned(),
                None,
                false,
                !busy,
                cx,
                |app, _, cx| {
                    if let Some(draft) = &mut app.automations.draft {
                        draft.choose_session = !draft.choose_session;
                        cx.notify();
                    }
                },
            ));
            if draft.choose_session {
                let mut list = column()
                    .id("schedule-target-options")
                    .w_full()
                    .max_h(px(180.))
                    .overflow_y_scroll()
                    .gap(px(4.));
                for session in self
                    .sessions
                    .iter()
                    .filter(|session| !self.archived_sessions.contains(&session.id))
                {
                    let id = session.id.clone();
                    list = list.child(action(
                        gpui::ElementId::Name(format!("schedule-target-{id}").into()),
                        session.title().to_owned(),
                        Some(Glyph::Comment),
                        false,
                        !busy,
                        cx,
                        move |app, _, cx| {
                            if let Some(draft) = &mut app.automations.draft {
                                draft.session = Some(id.clone());
                                draft.choose_session = false;
                                cx.notify();
                            }
                        },
                    ));
                }
                body = body.child(list);
            }
        } else {
            body = body.child(note(self.workspace.clone(), cx));
        }
        body.child(label("任务指令", cx))
            .child(schedule_field(&draft.prompt, 104., cx))
            .when_some(self.automations.mutation_error.as_ref(), |body, error| {
                body.child(automation_error(error, cx))
            })
            .child(row().w_full().justify_end().child(action(
                "schedule-save",
                if busy { "正在保存…" } else { "保存" },
                None,
                true,
                !busy && self.connected(),
                cx,
                |app, _, cx| app.save_automation_editor(cx),
            )))
    }

    pub(super) fn invalidate_automations(&mut self, cx: &mut Context<Self>) {
        self.automations.refresh = true;
        if self.page == WorkspacePage::Automations {
            self.read_automations(cx);
        }
    }

    fn open_automation_confirmation(
        &mut self,
        confirm: Confirmation,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.automations.mutation.is_some() {
            return;
        }
        let focus = cx.focus_handle();
        focus.focus(w, cx);
        self.automations.confirm = Some(confirm);
        self.automations.confirm_focus = Some(focus);
        self.automations.mutation_error = None;
        cx.notify();
    }

    fn close_automation_confirmation(&mut self, cx: &mut Context<Self>) {
        if self.automations.mutation.is_some() {
            return;
        }
        self.automations.confirm = None;
        self.automations.confirm_focus = None;
        self.automations.mutation_error = None;
        cx.notify();
    }

    fn read_automations(&mut self, cx: &mut Context<Self>) {
        if !self.connected()
            || self.automations.read.is_some()
            || self.automations.mutation.is_some()
        {
            return;
        }
        let request = Rc::new(());
        self.automations.read = Some(request.clone());
        self.automations.requested = true;
        self.automations.refresh = false;
        self.automations.error = None;
        cx.spawn(async move |this, cx| {
            let result = schedule_call(json!({"action":"catalog"}))
                .await
                .and_then(|value| {
                    serde_json::from_value::<Catalog>(value)
                        .map_err(|_| "无法读取自动化任务列表。".to_owned())
                });
            let _ = this.update(cx, |app, cx| {
                let state = &mut app.automations;
                if state
                    .read
                    .as_ref()
                    .is_none_or(|pending| !Rc::ptr_eq(pending, &request))
                {
                    return;
                }
                state.read = None;
                match result {
                    Ok(catalog) => {
                        if state
                            .detail
                            .as_ref()
                            .is_some_and(|id| !catalog.tasks.iter().any(|task| &task.id == id))
                        {
                            state.detail = None;
                            state.history = History::default();
                        }
                        state.catalog = Some(catalog);
                    }
                    Err(error) => state.error = Some(error),
                }
                if state.refresh && app.page == WorkspacePage::Automations {
                    app.read_automations(cx);
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn schedule_mutation(&mut self, operation: Value, cx: &mut Context<Self>) {
        if !self.connected() || self.automations.mutation.is_some() {
            return;
        }
        let creation = operation["action"] == "create";
        let running = operation["action"] == "run";
        let deleting = operation["action"] == "edit" && operation["request"]["action"] == "delete";
        let saving =
            creation || (operation["action"] == "edit" && self.automations.draft.is_some());
        let request = Rc::new(());
        self.automations.mutation = Some(request.clone());
        self.automations.mutation_error = None;
        cx.spawn(async move |this, cx| {
            let result =
                schedule_call(operation)
                    .await
                    .and_then(|value| match value["code"].as_str() {
                        Some("schedule_conflict") => {
                            Err("任务已变化，请刷新后重新编辑。".to_owned())
                        }
                        Some("schedule_not_found") => Err("任务已删除。".to_owned()),
                        Some("schedule_execution_pending") => {
                            Err("任务仍在投递或等待最短投递间隔。".to_owned())
                        }
                        Some(_) => Err("Host 未完成任务操作。".to_owned()),
                        None => Ok(value),
                    });
            let _ = this.update(cx, |app, cx| {
                let state = &mut app.automations;
                if state
                    .mutation
                    .as_ref()
                    .is_none_or(|pending| !Rc::ptr_eq(pending, &request))
                {
                    return;
                }
                state.mutation = None;
                match result {
                    Ok(value) => {
                        state.confirm = None;
                        state.confirm_focus = None;
                        if saving {
                            state.draft = None;
                        }
                        if deleting {
                            state.detail = None;
                            state.history = History::default();
                        }
                        if creation {
                            state.filter = None;
                            state.search = None;
                            state.detail = value["id"].as_str().map(str::to_owned);
                            state.show_history = false;
                            state.history = History::default();
                        }
                        if running {
                            state.history = History::default();
                            app.push_toast("任务已投递到会话。", cx);
                        }
                    }
                    Err(error) => state.mutation_error = Some(error),
                }
                if running
                    && app.automations.show_history
                    && app.automations.mutation_error.is_none()
                {
                    app.read_delivery_history(None, cx);
                }
                app.invalidate_automations(cx);
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    fn read_delivery_history(&mut self, before: Option<String>, cx: &mut Context<Self>) {
        let state = &mut self.automations;
        let Some(id) = state.detail.clone() else {
            return;
        };
        if state.history.request.is_some() {
            return;
        }
        let token = Rc::new(());
        state.history.request = Some(token.clone());
        state.history.error = None;
        cx.spawn(async move |this, cx| {
            let mut request = json!({"id":id, "limit":20});
            if let Some(cursor) = &before {
                request["before"] = json!(cursor);
            }
            let result = schedule_call(json!({"action":"history","request":request}))
                .await
                .and_then(|value| {
                    serde_json::from_value::<HistoryPage>(value)
                        .map_err(|_| "无法读取该任务的投递记录。".to_owned())
                });
            let _ = this.update(cx, |app, cx| {
                let history = &mut app.automations.history;
                if history
                    .request
                    .as_ref()
                    .is_none_or(|pending| !Rc::ptr_eq(pending, &token))
                {
                    return;
                }
                history.request = None;
                match result {
                    Ok(mut page) => {
                        if before.is_some()
                            && let Some(previous) = history.page.take()
                        {
                            let mut records = previous.records;
                            records.append(&mut page.records);
                            page.records = records;
                        }
                        history.page = Some(page);
                    }
                    Err(error) => history.error = Some(error),
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(super) fn render_automations_panel(
        &mut self,
        w: &mut Window,
        cx: &mut Context<Self>,
    ) -> Div {
        if !self.automations.requested || self.automations.refresh {
            self.read_automations(cx);
        }
        let connected = self.connected();
        self.arm_delivery_floor_refresh(cx);
        let p = dsh(cx);
        let state = &mut self.automations;
        let menu = state.create_menu.get_or_insert_with(|| CreateMenu {
            trigger: cx.focus_handle(),
            items: [cx.focus_handle(), cx.focus_handle()],
            open: false,
        });
        let (search, _) = state.search.get_or_insert_with(|| {
            let search = editor("搜索自动化任务".into(), InputMode::Single, w, cx);
            search.update(cx, |editor, _| {
                editor.set_placeholder("搜索自动化任务");
                editor.font_size = 13.;
            });
            let changes = cx.subscribe(&search, |_, _, event: &EditEvent, cx| {
                if *event == EditEvent::Changed {
                    cx.notify();
                }
            });
            (search, changes)
        });
        let search = search.clone();
        let query = search.read(cx).value().trim().to_lowercase();
        let available = state
            .catalog
            .as_ref()
            .is_some_and(|catalog| catalog.available);
        let can_create = state
            .catalog
            .as_ref()
            .is_some_and(|catalog| catalog.can_create_session)
            || self
                .sessions
                .iter()
                .any(|session| !self.archived_sessions.contains(&session.id));
        let chat_create = state
            .catalog
            .as_ref()
            .is_some_and(|catalog| catalog.conversation_tools);
        let busy = state.mutation.is_some();
        let tasks = state
            .catalog
            .as_ref()
            .map_or(&[][..], |catalog| &catalog.tasks);
        let count_ended = tasks
            .iter()
            .filter(|task| task.status == Status::Ended)
            .count();
        let show_detail = state.detail.is_some() || state.draft.is_some();
        let pane_width: f32 = w.viewport_size().width.into();
        let stacked = show_detail && pane_width < 980.;
        let mut tabs = row().w_full().gap(px(8.)).flex_wrap();
        for filter in [
            Some(Status::Active),
            Some(Status::Paused),
            Some(Status::Ended),
            None,
        ] {
            let selected = state.filter == filter;
            tabs = tabs.child(chip(
                gpui::ElementId::Name(
                    format!("schedule-filter-{}", filter.map_or("all", Status::label)).into(),
                ),
                filter.map_or("全部", Status::label),
                selected,
                available,
                cx,
                move |app, _, cx| {
                    app.automations.filter = filter;
                    cx.notify();
                },
            ));
        }
        let heading = row()
            .w_full()
            .gap(px(16.))
            .justify_between()
            .mb(px(24.))
            .child(
                div()
                    .text_size(px(20.))
                    .line_height(px(28.))
                    .font_weight(FontWeight::MEDIUM)
                    .child("自动化任务"),
            )
            .child(automation_create_controls(
                menu,
                available && !busy && can_create,
                chat_create,
                connected && !busy && !self.creating_session,
                w,
                cx,
            ));
        let search_row = row()
            .w_full()
            .gap(px(8.))
            .child(
                row()
                    .flex_1()
                    .min_w_0()
                    .h(px(36.))
                    .px(px(10.))
                    .gap(px(8.))
                    .rounded(px(12.))
                    .border(px(0.5))
                    .border_color(p.border_l3)
                    .child(icon(Glyph::Search, 14., p.label_tertiary))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .h_full()
                            .py(px(6.))
                            .child(search.clone()),
                    )
                    .when(!query.is_empty(), |search_row| {
                        search_row.child(super::dock_action_button(
                            row()
                                .id("schedule-search-clear")
                                .role(gpui::Role::Button)
                                .aria_label("清除搜索")
                                .tab_stop(true)
                                .size(px(22.))
                                .justify_center()
                                .rounded_full()
                                .cursor_pointer()
                                .hover(|style| style.bg(p.hover))
                                .child(icon(Glyph::Close, 12., p.label_secondary)),
                            cx.listener(move |_, _, w, cx| {
                                replace(&search, "", w, cx);
                                search.update(cx, |editor, cx| editor.focus(w, cx));
                            }),
                        ))
                    }),
            )
            .when(count_ended > 0, |row| {
                row.child(action(
                    "schedule-clear-ended",
                    "清除已结束任务",
                    None,
                    false,
                    available && !busy,
                    cx,
                    move |app, w, cx| {
                        app.open_automation_confirmation(
                            Confirmation::ClearEnded(count_ended),
                            w,
                            cx,
                        )
                    },
                ))
            });
        let mut body = column()
            .w_full()
            .max_w(px(960.))
            .mx_auto()
            .gap(px(14.))
            .child(heading)
            .child(tabs)
            .when(available, |body| body.child(search_row));
        if let Some(error) = &state.error {
            body = body.child(
                row()
                    .w_full()
                    .gap(px(12.))
                    .child(note(error.clone(), cx))
                    .child(action(
                        "schedule-retry",
                        "重试",
                        None,
                        false,
                        connected,
                        cx,
                        |app, _, cx| app.invalidate_automations(cx),
                    )),
            );
        } else if !available {
            body = body.child(note(
                if state.read.is_some() {
                    "正在加载自动化任务…"
                } else if !connected {
                    "连接 ACP 服务后可管理自动化任务。"
                } else {
                    "当前 ACP 服务未提供自动化任务。"
                },
                cx,
            ));
        }
        if available {
            let mut visible = tasks
                .iter()
                .filter(|task| {
                    state.filter.is_none_or(|filter| task.status == filter)
                        && (query.is_empty()
                            || task.title.to_lowercase().contains(&query)
                            || task.prompt.to_lowercase().contains(&query))
                })
                .peekable();
            if visible.peek().is_none() {
                let title = if !query.is_empty() {
                    "没有匹配的自动化任务"
                } else {
                    match state.filter {
                        Some(Status::Active) => "没有已开启的自动化任务",
                        Some(Status::Paused) => "没有已暂停的自动化任务",
                        Some(Status::Ended) => "没有已结束的自动化任务",
                        None => "还没有自动化任务",
                    }
                };
                body = body.child(
                    column()
                        .w_full()
                        .items_center()
                        .py(px(48.))
                        .child(
                            gpui::svg()
                                .data(if super::workspace::dark(cx) {
                                    include_bytes!("../../assets/schedule-empty-dark.svg")
                                        as &[u8]
                                } else {
                                    include_bytes!("../../assets/schedule-empty-light.svg")
                                        as &[u8]
                                })
                                .size(px(110.))
                                .mb(px(12.)),
                        )
                        .child(
                            div()
                                .text_size(px(14.))
                                .text_color(p.label_tertiary)
                                .mb(px(24.))
                                .child(title),
                        )
                        .child(action(
                            "schedule-empty-new",
                            "新建自动化任务",
                            None,
                            false,
                            !busy && can_create,
                            cx,
                            |app, w, cx| app.open_automation_editor(None, w, cx),
                        )),
                );
            }
            let mut rows = column().w_full().gap(px(2.));
            for task in visible {
                let id = task.id.clone();
                let selected = state.detail.as_deref() == Some(&id);
                let metadata = match task.status {
                    Status::Active => format!(
                        "{} · 下次运行 {}",
                        task.frequency(),
                        time(&task.scheduled_at)
                    ),
                    status => format!("{} · {}", status.label(), task.frequency()),
                };
                rows = rows.child(super::dock_action_button(
                    row()
                        .id(gpui::ElementId::Name(format!("schedule-task-{id}").into()))
                        .role(gpui::Role::Button)
                        .aria_label(task.title.clone())
                        .aria_expanded(selected)
                        .tab_stop(true)
                        .w_full()
                        .p(px(8.))
                        .items_start()
                        .gap(px(12.))
                        .rounded(px(12.))
                        .cursor_pointer()
                        .when(selected, |row| row.bg(p.hover))
                        .hover(|style| style.bg(p.hover))
                        .child(
                            div()
                                .pt(px(2.))
                                .child(icon(Glyph::Clock, 16., p.label_tertiary)),
                        )
                        .child(
                            column()
                                .flex_1()
                                .min_w_0()
                                .gap(px(2.))
                                .child(
                                    div()
                                        .truncate()
                                        .text_size(px(14.))
                                        .line_height(px(23.))
                                        .font_weight(FontWeight::MEDIUM)
                                        .child(task.title.clone()),
                                )
                                .child(note(metadata, cx)),
                        ),
                    cx.listener(move |app, _, _, cx| {
                        app.automations.detail = Some(id.clone());
                        app.automations.draft = None;
                        app.automations.show_history = false;
                        app.automations.history = History::default();
                        app.automations.mutation_error = None;
                        cx.notify();
                    }),
                ));
            }
            body = body.child(rows);
        }
        let viewport: f32 = w.viewport_size().width.into();
        let list = column()
            .id("automations-page")
            .flex_1()
            .min_w_0()
            .min_h_0()
            .h_full()
            .overflow_y_scroll()
            .px(px(if viewport < 768. {
                20.
            } else {
                (viewport * 0.04).clamp(24., 48.)
            }))
            .pt(px(if bridge::is_mac() { 58. } else { 28. }))
            .pb(px(48.))
            .child(body);
        let detail = if let Some(draft) = self.automations.draft.as_ref() {
            Some(self.automation_editor(draft, cx))
        } else {
            self.selected_automation()
                .map(|task| self.automation_detail(task, cx))
        };
        let mut panel = row()
            .relative()
            .size_full()
            .items_start()
            .min_w_0()
            .min_h_0()
            .on_key_down(cx.listener(|app, event: &gpui::KeyDownEvent, w, cx| {
                let state = &app.automations;
                if event.keystroke.key != "escape"
                    || w.default_prevented()
                    || state.confirm.is_some()
                    || (state.detail.is_none() && state.draft.is_none())
                {
                    return;
                }
                app.close_automation_detail(w, cx);
                w.prevent_default();
                cx.stop_propagation();
            }))
            .when(!stacked, |panel| panel.child(list))
            .when_some(detail, |panel, detail| {
                panel.child(
                    column()
                        .id("automation-detail-pane")
                        .h_full()
                        .min_w_0()
                        .min_h_0()
                        .overflow_y_scroll()
                        .when(stacked, |pane| pane.w_full())
                        .when(!stacked, |pane| {
                            pane.w(px(420.)).border_l(px(0.5)).border_color(p.border_l2)
                        })
                        .pt(px(if bridge::is_mac() { 58. } else { 28. }))
                        .px(px(20.))
                        .pb(px(24.))
                        .child(detail),
                )
            });
        if let Some(confirm) = &self.automations.confirm {
            panel = panel.child(self.automation_confirmation(confirm, cx));
        }
        panel
    }

    fn selected_automation(&self) -> Option<&Task> {
        let id = self.automations.detail.as_ref()?;
        self.automations
            .catalog
            .as_ref()?
            .tasks
            .iter()
            .find(|task| &task.id == id)
    }

    fn arm_delivery_floor_refresh(&mut self, cx: &mut Context<Self>) {
        let Some(end) = self
            .selected_automation()
            .and_then(|task| task.delivery_floor_ends_at.clone())
        else {
            return;
        };
        let remaining = js_sys::Date::parse(&end) - bridge::now_ms();
        if !remaining.is_finite()
            || remaining <= 0.
            || self.automations.floor_refresh.as_ref() == Some(&end)
        {
            return;
        }
        self.automations.floor_refresh = Some(end.clone());
        cx.spawn(async move |this, cx| {
            cx.background_executor()
                .timer(std::time::Duration::from_millis(remaining.ceil() as u64))
                .await;
            let _ = this.update(cx, |app, cx| {
                if app.automations.floor_refresh.as_ref() == Some(&end) {
                    app.automations.floor_refresh = None;
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn close_automation_detail(&mut self, w: &mut Window, cx: &mut Context<Self>) {
        if self.automations.mutation.is_some() {
            return;
        }
        self.automations.detail = None;
        if let Some(focus) = self
            .automations
            .draft
            .take()
            .and_then(|draft| draft.return_focus)
        {
            focus.focus(w, cx);
        }
        self.automations.history = History::default();
        self.automations.mutation_error = None;
        cx.notify();
    }

    fn automation_detail(&self, task: &Task, cx: &mut Context<Self>) -> Div {
        let state = &self.automations;
        let p = dsh(cx);
        let enabled = self.connected() && state.mutation.is_none() && state.error.is_none();
        let id = task.id.clone();
        let pause = task.edit(
            "update",
            Some(json!({"active":task.status != Status::Active})),
        );
        let delete = task.clone();
        let edit = task.clone();
        let running = task.execution_active;
        let wait = task
            .delivery_floor_ends_at
            .as_deref()
            .filter(|end| js_sys::Date::parse(end) > bridge::now_ms());
        let controls = row()
            .w_full()
            .gap(px(6.))
            .flex_wrap()
            .child(action(
                "schedule-edit",
                "编辑",
                None,
                false,
                enabled && !running,
                cx,
                move |app, w, cx| app.open_automation_editor(Some(edit.clone()), w, cx),
            ))
            .child(action(
                "schedule-pause",
                if task.status == Status::Active {
                    "暂停"
                } else {
                    "恢复"
                },
                None,
                false,
                enabled && !running && task.status != Status::Ended,
                cx,
                move |app, _, cx| app.schedule_mutation(pause.clone(), cx),
            ))
            .child(action(
                "schedule-run",
                "立即运行",
                None,
                false,
                enabled && !running && wait.is_none(),
                cx,
                move |app, _, cx| app.schedule_mutation(json!({"action":"run","id":id}), cx),
            ))
            .child(action(
                "schedule-delete",
                "删除",
                Some(Glyph::Trash),
                false,
                enabled && !running,
                cx,
                move |app, w, cx| {
                    app.open_automation_confirmation(Confirmation::Delete(delete.clone()), w, cx)
                },
            ));
        let mut body = column()
            .w_full()
            .gap(px(14.))
            .child(detail_header("任务详情", state.mutation.is_none(), cx))
            .child(
                row()
                    .w_full()
                    .gap(px(8.))
                    .child(chip(
                        "schedule-rule-tab",
                        "规则",
                        !state.show_history,
                        true,
                        cx,
                        |app, _, cx| {
                            app.automations.show_history = false;
                            cx.notify();
                        },
                    ))
                    .child(chip(
                        "schedule-history-tab",
                        "运行记录",
                        state.show_history,
                        true,
                        cx,
                        |app, _, cx| {
                            app.automations.show_history = true;
                            if app.automations.history.page.is_none() {
                                app.read_delivery_history(None, cx);
                            }
                            cx.notify();
                        },
                    )),
            )
            .child(
                div()
                    .text_size(px(20.))
                    .line_height(px(28.))
                    .font_weight(FontWeight::MEDIUM)
                    .child(task.title.clone()),
            )
            .child(controls)
            .when_some(state.mutation_error.as_ref(), |body, error| {
                body.child(automation_error(error, cx))
            });
        if state.show_history {
            return body.child(self.automation_history(cx));
        }
        body = body
            .child(note(
                if task.status == Status::Active {
                    format!("下次运行 {}", time(&task.scheduled_at))
                } else {
                    task.status.label().to_owned()
                },
                cx,
            ))
            .when(running, |body| body.child(note("任务正在投递或运行。", cx)))
            .when_some(wait, |body, wait| {
                body.child(note(format!("{} 后可再次运行", time(wait)), cx))
            })
            .child(label("运行频率", cx))
            .child(note(task.frequency(), cx))
            .child(label("任务指令", cx))
            .child(
                div()
                    .w_full()
                    .p(px(12.))
                    .rounded(px(12.))
                    .bg(p.bg_module_platform)
                    .text_size(px(14.))
                    .line_height(px(22.))
                    .child(task.prompt.clone()),
            )
            .child(label("投递目标", cx));
        body.child(match &task.target {
            Target::Session { session_id } => self.automation_session_link(session_id, cx),
            Target::NewOnce { .. } => note("首次运行时创建会话，后续继续使用该会话。", cx),
            Target::New => note("每次运行时创建新会话。", cx),
        })
    }

    fn automation_session_link(&self, id: &str, cx: &mut Context<Self>) -> Div {
        let session = self
            .sessions
            .iter()
            .find(|session| session.id == id)
            .cloned();
        if let Some(session) = session
            && !self.archived_sessions.contains(id)
        {
            let title = session.title().to_owned();
            row().w_full().child(action(
                "schedule-open-session",
                title,
                Some(Glyph::Comment),
                false,
                self.connected(),
                cx,
                move |app, w, cx| {
                    if app.active_session.as_deref() == Some(&session.id)
                        && app.conversation.acp_open
                    {
                        app.page = WorkspacePage::Conversation;
                        cx.notify();
                    } else {
                        app.resume(session.clone(), w, cx);
                    }
                },
            ))
        } else {
            note(
                if self.archived_sessions.contains(id) {
                    "目标会话已归档"
                } else {
                    "目标会话暂不可用"
                },
                cx,
            )
        }
    }

    fn automation_history(&self, cx: &mut Context<Self>) -> Div {
        let history = &self.automations.history;
        let p = dsh(cx);
        let mut body = column().w_full().gap(px(12.));
        if let Some(error) = &history.error {
            body = body.child(automation_error(error, cx)).child(action(
                "schedule-history-retry",
                "重试",
                None,
                false,
                true,
                cx,
                |app, _, cx| app.read_delivery_history(None, cx),
            ));
        }
        if let Some(page) = &history.page {
            if page.records.is_empty() {
                body = body.child(note("暂无投递记录", cx));
            }
            for delivery in &page.records {
                body = body.child(
                    column()
                        .w_full()
                        .gap(px(5.))
                        .p(px(10.))
                        .rounded(px(12.))
                        .bg(p.bg_module_platform)
                        .child(label(
                            &format!("已投递 {}", time(&delivery.delivered_at)),
                            cx,
                        ))
                        .child(note(
                            format!("计划时间 {}", time(&delivery.scheduled_at)),
                            cx,
                        ))
                        .when_some(delivery.prompt.as_ref(), |row, prompt| {
                            row.child(note(prompt.clone(), cx))
                        })
                        .child(self.automation_session_link(&delivery.target_session_id, cx)),
                );
            }
            if let Some(cursor) = page.next_before.as_ref() {
                let cursor = cursor.clone();
                body = body.child(action(
                    "schedule-history-more",
                    "加载更多",
                    None,
                    false,
                    history.request.is_none(),
                    cx,
                    move |app, _, cx| app.read_delivery_history(Some(cursor.clone()), cx),
                ));
            }
            if page.earlier_records_pruned || page.earlier_records_unavailable {
                body = body.child(note("更早的部分投递记录已超出保留范围或不可用。", cx));
            }
        }
        body.when(history.request.is_some(), |body| {
            body.child(note("正在读取投递记录…", cx))
        })
    }

    fn automation_confirmation(
        &self,
        confirm: &Confirmation,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        use super::permission_dialog::{dialog_action, dialog_header};
        let (title, description, operation) = match confirm {
            Confirmation::Delete(task) => (
                "删除自动化任务",
                format!("将删除「{}」及其投递记录。", task.title),
                task.edit("delete", None),
            ),
            Confirmation::ClearEnded(count) => (
                "清除已结束任务",
                format!("将删除 {count} 个已结束的任务及其投递记录。"),
                json!({"action":"clearEnded"}),
            ),
        };
        let busy = self.automations.mutation.is_some();
        let p = dsh(cx);
        row()
            .id("schedule-confirm-overlay")
            .absolute()
            .inset_0()
            .p(px(20.))
            .justify_center()
            .items_center()
            .bg(p.bg_mask_1)
            .occlude()
            .child(
                column()
                    .id("schedule-confirm-dialog")
                    .role(gpui::Role::Dialog)
                    .aria_label(title)
                    .tab_group()
                    .capture_key_down(cx.listener(|app, event: &gpui::KeyDownEvent, w, cx| {
                        if event.keystroke.key == "escape" {
                            app.close_automation_confirmation(cx);
                            w.prevent_default();
                            cx.stop_propagation();
                        } else {
                            super::dock_controls::traverse_dock_focus(event, w, cx);
                        }
                    }))
                    .w(px(380.))
                    .max_w_full()
                    .gap(px(12.))
                    .rounded(px(24.))
                    .bg(p.bg_layer_2)
                    .shadow(elevation_prominent())
                    .child(dialog_header(
                        title,
                        "schedule-confirm-close",
                        self.automations.confirm_focus.as_ref(),
                        cx,
                        |app, _, cx| app.close_automation_confirmation(cx),
                    ))
                    .child(
                        column()
                            .px(px(24.))
                            .gap(px(8.))
                            .child(note(description, cx))
                            .when_some(self.automations.mutation_error.as_ref(), |body, error| {
                                body.child(automation_error(error, cx))
                            }),
                    )
                    .child(
                        row()
                            .w_full()
                            .justify_end()
                            .gap(px(8.))
                            .px(px(24.))
                            .pb(px(24.))
                            .child(dialog_action(
                                "schedule-confirm-cancel",
                                "取消",
                                false,
                                busy,
                                0.,
                                cx,
                                |app, _, cx| app.close_automation_confirmation(cx),
                            ))
                            .child(dialog_action(
                                "schedule-confirm-apply",
                                if busy { "正在删除…" } else { "删除" },
                                true,
                                busy,
                                0.,
                                cx,
                                move |app, _, cx| app.schedule_mutation(operation.clone(), cx),
                            )),
                    ),
            )
    }
}

fn automation_create_controls(
    menu: &CreateMenu,
    manual: bool,
    chat: bool,
    connected: bool,
    w: &Window,
    cx: &mut Context<DshApp>,
) -> Div {
    let p = dsh(cx);
    let primary = action(
        "schedule-new",
        "新建",
        Some(Glyph::Plus),
        true,
        if chat { connected } else { manual },
        cx,
        move |app, w, cx| {
            if chat {
                app.start_automation_chat(w, cx);
            } else {
                app.open_automation_editor(None, w, cx);
            }
        },
    );
    if !chat {
        return row().child(primary);
    }
    let trigger = row()
        .id("schedule-create-method")
        .role(gpui::Role::Button)
        .aria_label("选择创建方式")
        .aria_expanded(menu.open && connected)
        .track_focus(&menu.trigger)
        .tab_stop(connected)
        .relative()
        .size(px(32.))
        .justify_center()
        .rounded_r(px(16.))
        .border_l(px(0.5))
        .border_color(p.label_primary_foreground.alpha(0.3))
        .bg(p.button_primary_fill)
        .child(icon(Glyph::Down, 12., p.label_primary_foreground));
    let trigger = if connected {
        trigger
            .cursor_pointer()
            .hover(|style| style.bg(p.button_primary_hover))
            .on_click(cx.listener(|app, _, w, cx| app.toggle_automation_create_menu(w, cx)))
            .capture_key_down(cx.listener(|app, event: &gpui::KeyDownEvent, w, cx| {
                let Some(menu) = &app.automations.create_menu else {
                    return;
                };
                // The deferred menu is still a descendant of the trigger in the focus tree.
                if menu.trigger.is_focused(w)
                    && !event.keystroke.modifiers.modified()
                    && matches!(event.keystroke.key.as_str(), "down" | "enter")
                    && !w.default_prevented()
                {
                    app.toggle_automation_create_menu(w, cx);
                    w.prevent_default();
                    cx.stop_propagation();
                }
            }))
    } else {
        trigger.opacity(0.45)
    };
    row().gap_0().child(primary.rounded_r(px(0.))).child(
        trigger.children(
            (menu.open && connected).then(|| automation_create_options(menu, manual, w, cx)),
        ),
    )
}

fn automation_create_options(
    menu: &CreateMenu,
    manual: bool,
    w: &Window,
    cx: &mut Context<DshApp>,
) -> impl IntoElement + use<> {
    use gpui::{AvailableSpace, Bounds, MouseDownEvent, Pixels, canvas, point, size};
    let p = dsh(cx);
    let bounds: Rc<Cell<Option<Bounds<Pixels>>>> = Rc::new(Cell::new(None));
    let anchor = bounds.clone();
    let mut card = column()
        .id("schedule-create-menu")
        .role(gpui::Role::Menu)
        .w(px(156.).min(w.viewport_size().width - px(24.)))
        .p(px(4.))
        .rounded(px(16.))
        .occlude()
        .child(super::menu_surface::menu_material(p, 16.))
        .shadow(super::menu_surface::menu_elevation(p))
        .on_mouse_down_out(cx.listener(move |app, event: &MouseDownEvent, _, cx| {
            if !bounds
                .get()
                .is_some_and(|bounds| bounds.contains(&event.position))
            {
                app.close_automation_create_menu();
                cx.notify();
            }
        }))
        .capture_key_down(cx.listener(move |app, event: &gpui::KeyDownEvent, w, cx| {
            let Some(menu) = &app.automations.create_menu else {
                return;
            };
            match event.keystroke.key.as_str() {
                "escape" | "tab" => {
                    menu.trigger.focus(w, cx);
                    app.close_automation_create_menu();
                    if event.keystroke.key == "tab" {
                        cx.notify();
                        return;
                    }
                }
                "home" => menu.items[0].focus(w, cx),
                "end" if manual => menu.items[1].focus(w, cx),
                "up" | "down" if manual => {
                    let index = usize::from(menu.items[0].is_focused(w));
                    menu.items[index].focus(w, cx);
                }
                _ => return,
            }
            w.prevent_default();
            cx.stop_propagation();
            cx.notify();
        }));
    for (index, (id, label, glyph, enabled)) in [
        ("schedule-create-chat", "在会话中创建", Glyph::Comment, true),
        ("schedule-create-manual", "手动创建", Glyph::Edit, manual),
    ]
    .into_iter()
    .enumerate()
    {
        let option = row()
            .id(id)
            .role(gpui::Role::MenuItem)
            .aria_label(label)
            .track_focus(&menu.items[index])
            .tab_stop(enabled)
            .w_full()
            .h(px(34.))
            .px(px(8.))
            .gap(px(10.))
            .rounded(px(12.))
            .text_size(px(13.))
            .child(icon(glyph, 14., p.label_secondary))
            .child(label);
        card = card.child(if enabled {
            super::dock_action_button(
                option
                    .cursor_pointer()
                    .hover(|style| style.bg(p.hover))
                    .focus_visible(|style| style.bg(p.hover)),
                cx.listener(move |app, _, w, cx| {
                    if index == 0 {
                        app.start_automation_chat(w, cx);
                    } else {
                        app.open_automation_editor(None, w, cx);
                    }
                }),
            )
        } else {
            option.opacity(0.45)
        });
    }
    let card = card.into_any_element();
    canvas(
        move |trigger, w, cx| {
            anchor.set(Some(trigger));
            let mut card = card;
            let measured = card.layout_as_root(
                size(AvailableSpace::MaxContent, AvailableSpace::MaxContent),
                w,
                cx,
            );
            let view = w.viewport_size();
            let x = (trigger.right() - measured.width).clamp(
                px(12.),
                (view.width - px(12.) - measured.width).max(px(12.)),
            );
            let y = (trigger.bottom() + px(4.))
                .min((view.height - px(12.) - measured.height).max(px(12.)));
            w.defer_draw(card, point(x.round(), y.round()), 4, None);
        },
        |_, _, _, _| {},
    )
    .absolute()
    .inset_0()
}

fn action(
    id: impl Into<gpui::ElementId>,
    label: impl Into<gpui::SharedString>,
    glyph: Option<Glyph>,
    primary: bool,
    enabled: bool,
    cx: &mut Context<DshApp>,
    on_click: impl Fn(&mut DshApp, &mut Window, &mut Context<DshApp>) + 'static,
) -> Stateful<Div> {
    let p = dsh(cx);
    let label = label.into();
    let color = if primary {
        p.label_primary_foreground
    } else {
        p.ink
    };
    let button = row()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label.clone())
        .tab_stop(enabled)
        .h(px(32.))
        .flex_none()
        .px(px(12.))
        .gap(px(6.))
        .rounded_full()
        .border(px(0.5))
        .border_color(if primary {
            p.button_primary_fill
        } else {
            p.border_l2
        })
        .when(primary, |button| button.bg(p.button_primary_fill))
        .text_size(px(13.))
        .line_height(px(20.))
        .text_color(color)
        .when_some(glyph, |button, glyph| button.child(icon(glyph, 13., color)))
        .child(div().max_w(px(260.)).truncate().child(label));
    if enabled {
        super::dock_action_button(
            button.cursor_pointer().hover(|style| {
                style.bg(if primary {
                    p.button_primary_hover
                } else {
                    p.hover
                })
            }),
            cx.listener(move |app, _, w, cx| on_click(app, w, cx)),
        )
    } else {
        button.opacity(0.45)
    }
}

fn chip(
    id: impl Into<gpui::ElementId>,
    label: &'static str,
    selected: bool,
    enabled: bool,
    cx: &mut Context<DshApp>,
    on_click: impl Fn(&mut DshApp, &mut Window, &mut Context<DshApp>) + 'static,
) -> Stateful<Div> {
    let p = dsh(cx);
    let chip = row()
        .id(id)
        .role(gpui::Role::Button)
        .aria_label(label)
        .aria_selected(selected)
        .tab_stop(enabled)
        .h(px(28.))
        .px(px(10.))
        .rounded_full()
        .text_size(px(14.))
        .line_height(px(22.))
        .text_color(if selected { p.ink } else { p.label_tertiary })
        .when(selected, |chip| chip.bg(p.hover))
        .child(label);
    if enabled {
        super::dock_action_button(
            chip.cursor_pointer().hover(|style| style.bg(p.hover)),
            cx.listener(move |app, _, w, cx| on_click(app, w, cx)),
        )
    } else {
        chip.opacity(0.45)
    }
}

fn detail_header(title: &'static str, enabled: bool, cx: &mut Context<DshApp>) -> Div {
    row()
        .w_full()
        .justify_between()
        .gap(px(12.))
        .child(label(title, cx))
        .child(action(
            "schedule-close-detail",
            "关闭",
            Some(Glyph::Close),
            false,
            enabled,
            cx,
            |app, w, cx| app.close_automation_detail(w, cx),
        ))
}

fn label(text: &str, cx: &App) -> Div {
    div()
        .text_size(px(14.))
        .line_height(px(20.))
        .font_weight(FontWeight::MEDIUM)
        .text_color(dsh(cx).ink)
        .child(text.to_owned())
}

fn automation_error(text: &str, cx: &App) -> Div {
    div()
        .w_full()
        .rounded(px(10.))
        .p(px(10.))
        .bg(dsh(cx).state_error_primary.alpha(0.10))
        .text_size(px(13.))
        .line_height(px(20.))
        .text_color(dsh(cx).state_error_primary)
        .child(text.to_owned())
}

fn automation_field(
    placeholder: &'static str,
    mode: InputMode,
    value: &str,
    w: &mut Window,
    cx: &mut Context<DshApp>,
) -> (Entity<Editor>, Subscription) {
    let editor = editor(placeholder.into(), mode, w, cx);
    editor.update(cx, |editor, _| {
        editor.set_placeholder(placeholder);
        editor.font_size = 13.;
    });
    replace(&editor, value, w, cx);
    let subscription = cx.subscribe(&editor, |_, _, event: &EditEvent, cx| {
        if *event == EditEvent::Changed {
            cx.notify();
        }
    });
    (editor, subscription)
}

fn schedule_field(editor: &Entity<Editor>, height: f32, cx: &App) -> Div {
    let p = dsh(cx);
    row()
        .w_full()
        .h(px(height))
        .px(px(10.))
        .py(px(6.))
        .rounded(px(12.))
        .border(px(0.5))
        .border_color(p.border_l3)
        .child(div().flex_1().min_w_0().h_full().child(editor.clone()))
}

async fn schedule_call(operation: Value) -> Result<Value, String> {
    bridge::invoke("schedule_tasks", json!({"operation":operation})).await
}

fn time(value: &str) -> String {
    let ms = js_sys::Date::parse(value);
    if ms.is_finite() {
        bridge::time_label(ms)
    } else {
        value.to_owned()
    }
}

fn clock_text(value: &str) -> &str {
    value
        .strip_suffix(":00.000")
        .or_else(|| value.strip_suffix(".000"))
        .unwrap_or(value)
}

fn local_zone() -> String {
    let format = js_sys::Intl::DateTimeFormat::new(&js_sys::Array::new(), &js_sys::Object::new());
    js_sys::Reflect::get(&format.resolved_options(), &"timeZone".into())
        .ok()
        .and_then(|value| value.as_string())
        .unwrap_or_else(|| "UTC".to_owned())
}

fn seconds(value: u64) -> String {
    if value >= 3600 && value.is_multiple_of(3600) {
        format!("{} 小时", value / 3600)
    } else if value >= 60 && value.is_multiple_of(60) {
        format!("{} 分钟", value / 60)
    } else {
        format!("{value} 秒")
    }
}

fn weekday_label(value: &Value) -> String {
    const DAYS: [&str; 7] = ["一", "二", "三", "四", "五", "六", "日"];
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|day| {
            day.as_u64()?
                .checked_sub(1)
                .and_then(|i| DAYS.get(i as usize))
                .copied()
        })
        .collect::<Vec<_>>()
        .join("、")
}
