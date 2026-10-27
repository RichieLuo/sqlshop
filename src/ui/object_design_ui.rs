//! Target-bound object designer, hosted in a modeless window. Its disposable
//! connection never changes the user's query transaction or active database.
use super::*;
use sqlshop::{
    advanced::AdvancedSource,
    db::Session,
    object_catalog::{CatalogObject, ObjectKind},
    object_design::{
        SchemaChange, SchemaSnapshot, editable_definition, object_plan, supported, template,
    },
};
use std::sync::{Arc, Mutex, mpsc};

type Shared = Arc<Mutex<Designer>>;
const KEY: &str = "object-design-window";
enum Reply {
    Schemas(Vec<SchemaSnapshot>),
    Saved(String),
}
enum Operation {
    ReadSchemas,
    Schema(SchemaChange, Vec<SchemaSnapshot>, String),
    Object(Box<CatalogObject>, Option<String>, String, Vec<String>),
}
struct Designer {
    source: AdvancedSource,
    object: Option<CatalogObject>,
    original: Option<String>,
    ddl: String,
    schemas: Vec<SchemaSnapshot>,
    schema_name: String,
    schema_owner: String,
    new_name: String,
    schema_action: u8,
    preview: Option<Vec<String>>,
    confirmed: bool,
    message: Option<String>,
    receiver: Option<mpsc::Receiver<Result<Reply, String>>>,
    kind: ObjectKind,
    name: String,
    parent: String,
    schema: String,
    needs_refresh: bool,
    saved: bool,
    dirty: bool,
    discard_requested: bool,
}
impl Designer {
    fn new(
        source: AdvancedSource,
        object: Option<CatalogObject>,
        original: Option<String>,
        schemas: bool,
    ) -> Self {
        let schema = object
            .as_ref()
            .map(|o| o.schema.clone())
            .or_else(|| source.target.schema.clone())
            .unwrap_or_else(|| match source.profile.engine {
                Engine::Postgres => "public".into(),
                Engine::MySql => source.target.database.clone().unwrap_or_default(),
                Engine::SqlServer => "dbo".into(),
            });
        let ddl = original
            .as_ref()
            .map(|sql| {
                object
                    .as_ref()
                    .and_then(|o| editable_definition(source.profile.engine, o, sql).ok())
                    .unwrap_or_else(|| sql.clone())
            })
            .unwrap_or_default();
        Self {
            source,
            object,
            original,
            ddl,
            schemas: Vec::new(),
            schema_name: String::new(),
            schema_owner: String::new(),
            new_name: String::new(),
            schema_action: if schemas { 0 } else { 255 },
            preview: None,
            confirmed: false,
            message: None,
            receiver: None,
            kind: ObjectKind::View,
            name: String::new(),
            parent: String::new(),
            schema,
            needs_refresh: false,
            saved: false,
            dirty: false,
            discard_requested: false,
        }
    }
    fn candidate(&self) -> CatalogObject {
        self.object.clone().unwrap_or_else(|| CatalogObject {
            schema: self.schema.clone(),
            name: self.name.clone(),
            kind: self.kind,
            parent: (!self.parent.is_empty()).then(|| self.parent.clone()),
            signature: None,
            owner: None,
            comment: None,
            estimated_rows: None,
            size_bytes: None,
            created_at: None,
            updated_at: None,
            engine: None,
        })
    }
    fn schema_change(&self) -> SchemaChange {
        match self.schema_action {
            0 => SchemaChange::Create {
                name: self.schema_name.clone(),
                owner: self.schema_owner.clone(),
            },
            1 => SchemaChange::Rename {
                name: self.schema_name.clone(),
                to: self.new_name.clone(),
            },
            2 => SchemaChange::Owner {
                name: self.schema_name.clone(),
                owner: self.schema_owner.clone(),
            },
            _ => SchemaChange::Drop {
                name: self.schema_name.clone(),
            },
        }
    }
    fn start(&mut self, credentials: Credentials, operation: Operation, ctx: egui::Context) {
        let source = self.source.clone();
        let (tx, rx) = mpsc::channel();
        self.receiver = Some(rx);
        self.message = None;
        self.confirmed = false;
        std::thread::spawn(move || {
            let result = (|| -> anyhow::Result<Reply> {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()?;
                runtime.block_on(async move {
                    anyhow::ensure!(
                        source.target.connection == source.profile.id,
                        "连接身份不符"
                    );
                    let database = source
                        .target
                        .database
                        .as_deref()
                        .ok_or_else(|| anyhow::anyhow!("数据库缺失"))?;
                    let mut bootstrap = source.profile.clone();
                    bootstrap.database = database.into();
                    let mut session = Session::connect(&bootstrap, &credentials)
                        .await
                        .map_err(|e| sqlshop::db::connection_error(&source.profile, e))?;
                    if session.current_database().await?.as_deref() != Some(database) {
                        session.select_database(database).await?;
                    }
                    anyhow::ensure!(
                        session.current_database().await?.as_deref() == Some(database),
                        "服务器数据库与绑定目标不符，未执行"
                    );
                    match operation {
                        Operation::ReadSchemas => {
                            Ok(Reply::Schemas(session.design_schemas().await?))
                        }
                        Operation::Schema(change, expected, preview) => {
                            session
                                .apply_schema_design(&change, &expected, &preview)
                                .await?;
                            Ok(Reply::Saved("Schema 已保存".into()))
                        }
                        Operation::Object(object, expected, draft, preview) => Ok(Reply::Saved(
                            session
                                .apply_object_design(&object, expected.as_deref(), &draft, &preview)
                                .await?,
                        )),
                    }
                })
            })();
            let _ = tx.send(result.map_err(|e| format!("{e:#}")));
            ctx.request_repaint();
        });
    }
}
impl App {
    /// A detached designer connection must finish before native app shutdown.
    pub(super) fn object_design_busy(&self) -> bool {
        self.context
            .data(|data| data.get_temp::<Shared>(egui::Id::new(KEY)))
            .is_some_and(|state| {
                state
                    .lock()
                    .map(|doc| doc.receiver.is_some())
                    .unwrap_or(true)
            })
    }

    /// Includes modeless drafts, which are not owned by a QueryTab.
    pub(super) fn object_design_dirty(&self) -> bool {
        self.context
            .data(|data| data.get_temp::<Shared>(egui::Id::new(KEY)))
            .is_some_and(|state| state.lock().map(|doc| doc.dirty).unwrap_or(true))
    }

    pub(super) fn open_object_designer(
        &mut self,
        source: AdvancedSource,
        object: Option<CatalogObject>,
        original: Option<String>,
        schemas: bool,
    ) {
        let occupied = self
            .context
            .data(|d| d.get_temp::<Shared>(egui::Id::new(KEY)))
            .is_some();
        if occupied {
            self.fail("请先关闭当前对象设计器；未保存的草稿保留在设计器中".into());
            return;
        }
        let mut doc = Designer::new(source, object, original, schemas);
        if doc
            .object
            .as_ref()
            .is_some_and(|o| o.kind == ObjectKind::Sequence)
        {
            doc.ddl = format!(
                "ALTER SEQUENCE {}.{} INCREMENT BY 1;",
                doc.source.profile.engine.quote(&doc.candidate().schema),
                doc.source.profile.engine.quote(&doc.candidate().name)
            );
        }
        self.context
            .data_mut(|d| d.insert_temp(egui::Id::new(KEY), Arc::new(Mutex::new(doc))));
    }
    pub(super) fn object_design_window(&mut self) {
        let Some(shared) = self
            .context
            .data(|d| d.get_temp::<Shared>(egui::Id::new(KEY)))
        else {
            return;
        };
        let Ok(mut doc) = shared.lock() else {
            return;
        };
        let reply = doc.receiver.as_ref().and_then(|rx| match rx.try_recv() {
            Ok(reply) => Some(reply),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                "执行线程已退出，服务器结果未知；草稿已保留，请先刷新服务器状态再决定是否重试"
                    .into(),
            )),
        });
        if let Some(reply) = reply {
            doc.receiver = None;
            match reply {
                Ok(Reply::Schemas(schemas)) => {
                    doc.schemas = schemas;
                    doc.message = Some("Schema 属性已读取".into());
                }
                Ok(Reply::Saved(message)) => {
                    doc.message = Some(format!(
                        "{message}。请关闭设计器并重新读取服务器定义后继续编辑。"
                    ));
                    doc.needs_refresh = true;
                    doc.preview = None;
                    doc.saved = doc.schema_action == 255;
                    doc.dirty = false;
                    doc.discard_requested = false;
                }
                Err(error) => doc.message = Some(error),
            }
        }
        let busy = doc.receiver.is_some();
        let mut open = true;
        let mut operation = None;
        let mut close = false;
        dialog_window(if doc.schema_action==255{"对象设计器"}else{"Schema / 数据库管理"}, &self.context).id(egui::Id::new(KEY)).open(&mut open).default_width(850.).resizable(true).show(&self.context,|ui|{
            let engine=doc.source.profile.engine;
            ui.strong(format!("{} · {}:{} · {} · {}",doc.source.profile.name,doc.source.profile.host,doc.source.profile.port,doc.source.target.database.as_deref().unwrap_or_default(),engine.label()));
            ui.small("连接和数据库已绑定；执行使用独立连接。修改连接配置后必须重新打开设计器。");
            let before=(doc.ddl.clone(),doc.schema.clone(),doc.name.clone(),doc.parent.clone(),doc.kind,doc.schema_name.clone(),doc.schema_owner.clone(),doc.new_name.clone(),doc.schema_action);
            ui.add_enabled_ui(!busy&&!doc.saved,|ui|{
                if doc.schema_action!=255 {
                    if engine==Engine::MySql {ui.label("MySQL 的 Schema 是数据库。此处支持新建数据库；不支持改名或所有者。删除数据库必须使用明确的数据删除流程。");}
                    if ui.button("读取 Schema / 数据库属性").clicked(){operation=Some(Operation::ReadSchemas);}
                    let schemas=doc.schemas.clone();
                    egui::ScrollArea::vertical().id_salt("design-schemas").max_height(150.).show(ui,|ui|{
                        for schema in schemas {if ui.selectable_label(doc.schema_name==schema.name,format!("{} · {}",schema.name,schema.owner)).clicked(){doc.schema_name=schema.name;doc.schema_owner=schema.owner;}}
                    });
                    ui.horizontal(|ui|{ui.selectable_value(&mut doc.schema_action,0,"新建");if engine!=Engine::MySql {if engine==Engine::Postgres{ui.selectable_value(&mut doc.schema_action,1,"改名");}ui.selectable_value(&mut doc.schema_action,2,"所有者");ui.selectable_value(&mut doc.schema_action,3,"删除空 Schema");}});
                    ui.horizontal(|ui|{ui.label(if engine==Engine::MySql{"数据库名称"}else{"Schema 名称"});ui.text_edit_singleline(&mut doc.schema_name);});
                    if doc.schema_action==1{ui.horizontal(|ui|{ui.label("新名称");ui.text_edit_singleline(&mut doc.new_name);});}
                    if engine!=Engine::MySql&&matches!(doc.schema_action,0|2){ui.horizontal(|ui|{ui.label("所有者 / 授权主体");ui.text_edit_singleline(&mut doc.schema_owner);});}
                } else {
                    if doc.object.is_none() {
                        egui::ComboBox::from_id_salt("object-design-kind").selected_text(doc.kind.label()).show_ui(ui,|ui|{for kind in ObjectKind::ALL{if supported(engine,kind,false){ui.selectable_value(&mut doc.kind,kind,kind.label());}}});
                        ui.horizontal(|ui|{ui.label("Schema");ui.text_edit_singleline(&mut doc.schema);ui.label("对象名");ui.text_edit_singleline(&mut doc.name);});
                        if doc.kind==ObjectKind::Trigger{ui.horizontal(|ui|{ui.label("所属表");ui.text_edit_singleline(&mut doc.parent);});}
                        if ui.button("生成定义模板（替换当前输入）").clicked(){doc.ddl=template(engine,&doc.candidate());}
                    } else {ui.label(format!("{} · {}",doc.candidate().display_name(),doc.candidate().kind.label()));}
                    ui.label("编辑完整定义，使用目标 Schema.名称。保存前会重新读取定义并检查对象身份。PostgreSQL 例程参数身份不可在此改变。");
                    if engine==Engine::MySql&&doc.original.is_some()&&matches!(doc.candidate().kind,ObjectKind::Function|ObjectKind::Procedure|ObjectKind::Trigger){ui.colored_label(ui.visuals().warn_fg_color,"MySQL 修改此对象需要 DROP + CREATE，存在不可回滚的间隔；重建失败时旧对象可能已删除，授权需另行核对。请仔细审核下方两条 SQL。");}
                    if doc.candidate().kind==ObjectKind::Sequence{ui.label("序列修改使用 ALTER SEQUENCE；只有预览中明确列出的参数会改变。");}
                    if ui.button("复制当前草稿").clicked(){ui.ctx().copy_text(doc.ddl.clone());}
                    if let Some(original)=&doc.original {ui.collapsing("打开设计器时的服务器定义",|ui|{if ui.button("复制原始定义").clicked(){ui.ctx().copy_text(original.clone());}ui.add(egui::Label::new(RichText::new(original).monospace()).selectable(true).wrap());});}
                    egui::ScrollArea::vertical().max_height(350.).show(ui,|ui|{ui.add(egui::TextEdit::multiline(&mut doc.ddl).code_editor().desired_rows(13).desired_width(f32::INFINITY));});
                }
            });
            let after=(doc.ddl.clone(),doc.schema.clone(),doc.name.clone(),doc.parent.clone(),doc.kind,doc.schema_name.clone(),doc.schema_owner.clone(),doc.new_name.clone(),doc.schema_action);
            if before!=after {doc.preview=None;doc.confirmed=false;doc.dirty=true;}
            if ui.add_enabled(!busy&&!doc.saved,egui::Button::new("生成 SQL 预览")).clicked(){let plan=if doc.schema_action==255{object_plan(engine,&doc.candidate(),&doc.ddl,doc.original.is_some())}else{doc.schema_change().sql(engine).map(|sql|vec![sql])};match plan{Ok(plan)=>{doc.preview=Some(plan);doc.confirmed=false;},Err(e)=>doc.message=Some(format!("{e:#}"))}}
            if let Some(preview)=doc.preview.clone(){ui.separator();ui.label("即将执行的 SQL");ui.add(egui::Label::new(RichText::new(preview.join("\n\n")).monospace()).selectable(true).wrap());ui.add_enabled(!busy,egui::Checkbox::new(&mut doc.confirmed,"确认对上述连接、数据库及对象执行这些更改"));if ui.add_enabled(!busy&&doc.confirmed,egui::Button::new("执行已确认 SQL")).clicked(){operation=Some(if doc.schema_action==255{Operation::Object(Box::new(doc.candidate()),doc.original.clone(),doc.ddl.clone(),preview)}else{Operation::Schema(doc.schema_change(),doc.schemas.clone(),preview[0].clone())});}}
            if busy{ui.spinner();ui.label("服务器操作进行中，请等待执行结果。");}
            if let Some(message)=&doc.message{ui.add(egui::Label::new(message).selectable(true).wrap());}
            if ui.add_enabled(!busy,egui::Button::new("关闭设计器")).clicked(){close=true;}
        });
        if let Some(operation) = operation {
            let current = self.profiles.iter().find(|p| p.id == doc.source.profile.id);
            if current.and_then(|p| serde_json::to_value(p).ok())
                != serde_json::to_value(&doc.source.profile).ok()
            {
                doc.message = Some("连接配置已改变；请保留草稿并重新打开设计器".into());
            } else {
                match self.credentials_for(&doc.source.profile) {
                    Ok(Some(credentials)) => {
                        doc.start(credentials, operation, self.context.clone())
                    }
                    Ok(None) => doc.message = Some("请先连接并提供凭据".into()),
                    Err(e) => doc.message = Some(format!("读取凭据失败：{e:#}")),
                }
            }
        }
        if doc.needs_refresh {
            if let Some(object) = &doc.object {
                for definition in self
                    .tabs
                    .iter_mut()
                    .filter_map(|tab| tab.definition.as_mut())
                {
                    if definition.connection == doc.source.profile.id
                        && definition.database
                            == doc.source.target.database.as_deref().unwrap_or_default()
                        && super::object_browser_host::same_object(&definition.object, object)
                    {
                        definition.stale = true;
                    }
                }
            }
            let connection = doc.source.profile.id;
            let database = doc.source.target.database.clone().unwrap_or_default();
            if !self.session_busy(connection)
                && self
                    .sessions
                    .get(&connection)
                    .is_some_and(|s| s.active_database.as_deref() == Some(database.as_str()))
            {
                let request =
                    if doc.schema_action != 255 && doc.source.profile.engine == Engine::MySql {
                        Request::Databases
                    } else {
                        Request::ObjectCatalog { database }
                    };
                self.request_on(connection, request, "正在刷新对象目录…");
                if doc.schema_action != 255 && doc.source.profile.engine == Engine::Postgres {
                    for state in self
                        .query_sessions
                        .values_mut()
                        .filter(|state| state.profile.id == connection)
                    {
                        state.schemas_target = None;
                    }
                }
                doc.needs_refresh = false;
            }
        }
        let mut discard = false;
        let busy = doc.receiver.is_some();
        if close || !open {
            if busy {
                doc.message =
                    Some("服务器操作尚未结束，设计器和草稿会保留；请等待结果后再关闭".into());
            } else if doc.dirty {
                doc.discard_requested = true;
            } else {
                discard = true;
            }
        }
        if doc.discard_requested {
            dialog_window("丢弃未保存的设计草稿？", &self.context)
                .id(egui::Id::new("object-design-discard-confirm"))
                .collapsible(false)
                .resizable(false)
                .show(&self.context, |ui| {
                    ui.label("当前对象定义或 Schema 设置尚未保存。关闭后将丢弃这份草稿。");
                    dialog_actions(ui, |ui| {
                        if ui
                            .add_enabled(!busy, danger_button(ui, "丢弃草稿并关闭"))
                            .clicked()
                        {
                            discard = true;
                        }
                        if ui.button("继续编辑").clicked() {
                            doc.discard_requested = false;
                        }
                    });
                });
        }
        if discard && !busy {
            self.context
                .data_mut(|data| data.remove::<Shared>(egui::Id::new(KEY)));
        }
        if busy {
            self.context
                .request_repaint_after(std::time::Duration::from_millis(250));
        }
    }
}
