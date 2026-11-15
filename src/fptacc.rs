//! Аккаунт FunPay Tools в приложении: вход через Telegram-бота, PRO, цветные ники, рамка и баннер.
//! Тот же сервер, что у вкладки «Аккаунт» в расширении (воркер fpt-promo, задания выполняет бот).
//! Рамки у аватарок и баннер профиля рисуются по decor.json (см. decor.rs).

use crate::decor;
use crate::ui::{post, Ui};
use crate::worker::Cmd;
use crate::{AccNick, AccPlan, DecorBanner, Ring};
use serde_json::{json, Value};
use slint::{Image, Model, ModelRc, VecModel};
use std::sync::atomic::{AtomicU64, Ordering};
use std::rc::Rc;
use std::time::{Duration, Instant};

pub const API: &str = "https://fpt-promo.starobinskiy01.workers.dev";
const FRAME_PX: u32 = 256;
const BANNER_PX: u32 = 720;

/// номера текущего входа и текущего счёта: старые потоки видят, что их уже сменили, и выходят
static LOGIN_GEN: AtomicU64 = AtomicU64::new(0);
static PAY_GEN: AtomicU64 = AtomicU64::new(0);

/// Что сделать, когда аккаунт FunPay подтверждён
#[derive(Clone, Debug)]
pub enum After {
    Nick(String),
    Decor(i32, i32),
}

fn model<T: Clone + 'static>(v: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(v)))
}

// ------------------------------------------------------------ сервер

pub fn api(path: &str, body: Value) -> Value {
    let client = crate::fp::http_client(30);
    match client.post(format!("{API}{path}")).json(&body).send() {
        Ok(r) => {
            let status = r.status();
            let mut v: Value = r.json().unwrap_or_else(|_| json!({}));
            if !status.is_success() && v.get("error").is_none() {
                v["error"] = json!(format!("HTTP_{}", status.as_u16()));
            }
            v
        }
        Err(e) => {
            crate::log::write(&format!("fpt-promo {path}: {e}"));
            json!({ "error": "NET" })
        }
    }
}

fn ok(v: &Value) -> bool {
    v.get("ok").and_then(|x| x.as_bool()).unwrap_or(false)
}

fn err(v: &Value) -> String {
    v.get("error").and_then(|x| x.as_str()).unwrap_or("UNKNOWN").to_string()
}

/// Задание боту через воркер: создать и дождаться ответа (до 40 секунд).
pub fn job(session: &str, kind: &str, payload: Value) -> Result<Value, String> {
    let c = api("/job", json!({ "session": session, "type": kind, "payload": payload }));
    if !ok(&c) {
        return Err(err(&c));
    }
    let id = c.get("id").cloned().unwrap_or(Value::Null);
    let deadline = Instant::now() + Duration::from_secs(40);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(1200));
        let r = api("/job/result", json!({ "session": session, "id": id }));
        if ok(&r) && r.get("pending").and_then(|x| x.as_bool()).unwrap_or(false) {
            continue;
        }
        if !ok(&r) {
            return Err(err(&r));
        }
        let res = r.get("result").cloned().unwrap_or(Value::Null);
        if !res.get("ok").and_then(|x| x.as_bool()).unwrap_or(false) {
            return Err(res.get("error").and_then(|x| x.as_str()).unwrap_or("JOB_FAILED").to_string());
        }
        return Ok(res);
    }
    Err("BOT_TIMEOUT".into())
}

pub fn err_text(code: &str) -> String {
    let t = match code {
        "NET" => "Нет связи с сервером FunPay Tools. Проверь интернет или прокси.",
        "PAYMENT_METHOD_UNAVAILABLE" => "Этот способ оплаты сейчас недоступен, выбери другой.",
        "NEED_PAYMENT" => "Сначала оплати ник: бесплатный ник положен только с PRO и только первый.",
        "NICK_TAKEN" => "Этот ник уже украшен другим человеком. Напиши в поддержку @AlliSighs.",
        "INVOICE_NOT_FOUND" => "Счёт не найден, создай новый.",
        "NOT_YOUR_NICK" => "Этот ник записан не на твой Telegram.",
        "TOO_FAST" => "Слишком быстро, подожди секунду.",
        "BOT_TIMEOUT" => "Бот не ответил. Возможно, он перезапускается, попробуй через минуту.",
        "NO_SESSION" => "Нужно заново подтвердить аккаунт FunPay.",
        "NO_TG_SESSION" => "Вход через Telegram устарел, войди ещё раз.",
        "BAD_KEY" => "Ключ не похож на FPT-STYLE. Скопируй его в генераторе ещё раз.",
        "TOO_MANY_CHANGES" => "Стиль меняли слишком часто.",
        "NEED_FRAME" => "Рамка не куплена и PRO нет.",
        "NEED_BANNER" => "Баннер не куплен и PRO нет.",
        "VERIFY_TIMEOUT" => "FunPay долго не показывает проверочный лот. Попробуй через минуту.",
        "LOGIN_TIMEOUT" => "Не дождался подтверждения в боте. Попробуй ещё раз.",
        _ => "",
    };
    if !t.is_empty() {
        return t.to_string();
    }
    if let Some(r) = code.strip_prefix("DONATERS_") {
        return format!("Сервер ников не принял стиль: {r}");
    }
    format!("Ошибка: {code}")
}

fn product_name(p: &str) -> &'static str {
    match p {
        "1m" => "PRO на 1 месяц",
        "3m" => "PRO на 3 месяца",
        "forever" => "PRO навсегда",
        "nick" => "Цветной ник навсегда",
        "frame" => "Рамка для аватарки навсегда",
        "banner" => "Баннер навсегда",
        _ => "Покупка",
    }
}

fn s(v: &Value, k: &str) -> String {
    match v.get(k) {
        Some(Value::String(x)) => x.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn n(v: &Value, k: &str) -> i64 {
    match v.get(k) {
        Some(Value::Number(x)) => x.as_i64().or_else(|| x.as_f64().map(|f| f as i64)).unwrap_or(0),
        Some(Value::String(x)) => x.trim().parse().unwrap_or(0),
        Some(Value::Bool(b)) => *b as i64,
        _ => 0,
    }
}

// ------------------------------------------------------------ рамки и баннеры

impl Ui {
    /// Пустые списки под картинки и замеры колец; сами картинки грузятся, когда понадобятся.
    pub fn decor_init(&mut self) {
        let rings: Vec<Ring> = decor::RING.iter().map(|r| Ring { k: r.0, dx: r.1, dy: r.2 }).collect();
        let banners: Vec<DecorBanner> = decor::BANNERS
            .iter()
            .map(|b| DecorBanner { id: b.0, name: b.1.into(), img: Image::default(), has: false })
            .collect();
        let n = decor::FRAMES.len();
        self.st(|st| {
            st.set_decor_rings(model(rings));
            st.set_decor_frames(model(vec![Image::default(); n]));
            st.set_decor_has(model(vec![false; n]));
            st.set_decor_banners(model(banners));
        });
    }

    /// Поставить картинку рамки (kind 0) или баннера (kind 1) в очередь; если уже есть, сразу показать.
    pub fn decor_request(&mut self, kind: u8, id: i32) {
        if id <= 0 {
            return;
        }
        let url = if kind == 0 { decor::frame_url(id) } else { decor::banner_url(id) };
        if url.is_empty() {
            return;
        }
        let (key, img) = self.img(&url, if kind == 0 { FRAME_PX } else { BANNER_PX });
        self.ext.decor_keys.insert(key, (kind, id));
        if let Some((img, _, _)) = img {
            self.decor_put(kind, id, img);
        }
    }

    fn decor_put(&mut self, kind: u8, id: i32, img: Image) {
        let i = (id - 1) as usize;
        self.st(|st| {
            if kind == 0 {
                let fr = st.get_decor_frames();
                if i < fr.row_count() {
                    fr.set_row_data(i, img.clone());
                    st.get_decor_has().set_row_data(i, true);
                }
            } else {
                let bs = st.get_decor_banners();
                for r in 0..bs.row_count() {
                    if let Some(mut b) = bs.row_data(r) {
                        if b.id == id && !b.has {
                            b.img = img.clone();
                            b.has = true;
                            bs.set_row_data(r, b);
                        }
                    }
                }
            }
        });
        if kind == 1 {
            self.decor_pf_banner();
        }
    }

    /// Хук из image_ready: пришла картинка, возможно это рамка или баннер.
    pub fn decor_image_ready(&mut self, key: &str, img: &Image) {
        if let Some((kind, id)) = self.ext.decor_keys.get(key).copied() {
            self.decor_put(kind, id, img.clone());
        }
    }

    /// Рамка пользователя (и картинка для неё в очередь).
    pub fn user_frame(&mut self, uid: &str) -> i32 {
        let f = decor::active(uid).0;
        if f > 0 {
            self.decor_request(0, f);
        }
        f
    }

    /// Собеседник в личном чате: в списке чатов есть только ник, id ищем по нему в decor.json.
    pub fn chat_frame(&mut self, name: &str) -> i32 {
        let uid = decor::uid_by_name(name);
        if uid.is_empty() || uid == self.me.id {
            return 0;
        }
        self.user_frame(&uid)
    }

    /// Рамки у своей аватарки, в профиле и в карточке собеседника, баннер в профиле.
    pub fn decor_heads(&mut self) {
        let me = self.me.id.clone();
        let pf = self.st(|st| st.get_pf_id().to_string()).unwrap_or_default();
        let card = self.ext.card_uid.clone();
        let (mf, pff, cf) = (self.user_frame(&me), self.user_frame(&pf), self.user_frame(&card));
        self.st(|st| {
            st.set_me_frame(mf);
            st.set_pf_frame(pff);
            st.set_card_frame(cf);
        });
        self.decor_pf_banner();
    }

    /// Баннер FunPay Tools в профиле, если у человека нет баннера из описания.
    pub fn decor_pf_banner(&mut self) {
        if !self.ext.banner_url.is_empty() {
            return;
        }
        let pf = self.st(|st| st.get_pf_id().to_string()).unwrap_or_default();
        let b = decor::active(&pf).1;
        if b <= 0 {
            let was = self.st(|st| st.get_pf_banner_ar() > 0.0).unwrap_or(false);
            if was {
                self.st(|st| {
                    st.set_pf_has_banner(false);
                    st.set_pf_banner_ar(0.0);
                });
            }
            return;
        }
        let img = self
            .st(|st| {
                let bs = st.get_decor_banners();
                (0..bs.row_count()).filter_map(|r| bs.row_data(r)).find(|x| x.id == b && x.has).map(|x| x.img)
            })
            .flatten();
        match img {
            Some(img) => self.st(|st| {
                st.set_pf_banner(img);
                st.set_pf_has_banner(true);
                st.set_pf_banner_ar(decor::banner_ar(b));
            }),
            None => {
                self.decor_request(1, b);
                None
            }
        };
    }

    /// Пришёл свежий decor.json: обновляем всё, где видны аватарки.
    pub fn decor_loaded(&mut self) {
        self.decor_heads();
        self.rebuild_chats();
        self.rebuild_msgs();
    }

    // ------------------------------------------------------------ окно аккаунта

    fn acc_msg(&mut self, text: &str, is_err: bool) {
        let t = text.to_string();
        self.st(|st| {
            st.set_acc_msg(t.into());
            st.set_acc_msg_err(is_err);
        });
    }

    fn decor_msg(&mut self, text: &str, is_err: bool) {
        let t = text.to_string();
        self.st(|st| {
            st.set_acc_decor_msg(t.into());
            st.set_acc_decor_err(is_err);
        });
    }

    fn tg_session(&self) -> String {
        self.cfg.lock().unwrap().fpt_tg.0.clone()
    }

    pub fn acc_show(&mut self) {
        for f in decor::FRAMES {
            self.decor_request(0, f.0);
        }
        for b in decor::BANNERS {
            self.decor_request(1, b.0);
        }
        let (f, b) = decor::active(&self.me.id);
        let (sess, label) = self.cfg.lock().unwrap().fpt_tg.clone();
        self.st(|st| {
            st.set_acc_df(f);
            st.set_acc_db(b);
            st.set_acc_msg("".into());
            st.set_acc_decor_msg("".into());
            st.set_acc_tg(!sess.is_empty());
            st.set_acc_tg_name(label.into());
            st.set_acc_open(true);
        });
        if !sess.is_empty() && self.ext.acc.is_none() {
            self.acc_reload();
        } else if self.ext.acc.is_some() {
            self.acc_render();
        }
    }

    pub fn acc_reload(&mut self) {
        let sess = self.tg_session();
        if sess.is_empty() {
            return;
        }
        self.st(|st| st.set_acc_loading(true));
        std::thread::spawn(move || {
            let r = job(&sess, "me", json!({}));
            post(move |u| u.acc_profile(r));
        });
    }

    fn acc_profile(&mut self, r: Result<Value, String>) {
        self.st(|st| st.set_acc_loading(false));
        match r {
            Ok(v) => {
                self.ext.acc = Some(v);
                self.acc_render();
            }
            Err(e) => {
                if e == "NO_TG_SESSION" {
                    self.acc_forget_tg();
                }
                self.st(|st| st.set_acc_pro("".into()));
                self.acc_msg(&err_text(&e), true);
            }
        }
    }

    fn acc_forget_tg(&mut self) {
        {
            let mut c = self.cfg.lock().unwrap();
            c.fpt_tg = (String::new(), String::new());
            c.save();
        }
        self.ext.acc = None;
        self.st(|st| {
            st.set_acc_tg(false);
            st.set_acc_pro("".into());
            st.set_acc_pay_step(0);
            st.set_acc_pay_product("".into());
        });
    }

    /// Профиль из бота → магазин, ники и права на украшения.
    fn acc_render(&mut self) {
        let Some(p) = self.ext.acc.clone() else { return };
        let dark = self.dark();
        let pro = match s(&p, "pro").as_str() {
            "forever" => "навсегда".to_string(),
            "temporary" => {
                let until = n(&p, "proUntil");
                if until > 0 {
                    chrono::DateTime::from_timestamp_millis(until)
                        .map(|d| format!("до {}", d.with_timezone(&chrono::Local).format("%d.%m.%Y")))
                        .unwrap_or_else(|| "активна".into())
                } else {
                    "активна".into()
                }
            }
            _ => "нет".into(),
        };
        let nicks = p.get("nicks").and_then(|x| x.as_array()).cloned().unwrap_or_default();
        let me = self.me.name.clone();
        let me_owned = nicks.iter().any(|x| s(x, "nickname").to_lowercase() == me.to_lowercase());
        let credits = n(&p, "nickCredits");
        let free = p.get("freeNick").and_then(|x| x.as_bool()).unwrap_or(false);
        let can_add = !me.is_empty() && !me_owned && (free || credits > 0);
        let mut plans: Vec<AccPlan> = Vec::new();
        if let Some(pl) = p.get("plans").and_then(|x| x.as_object()) {
            for k in ["1m", "3m", "forever"] {
                if let Some(x) = pl.get(k) {
                    plans.push(AccPlan {
                        key: k.into(),
                        label: s(x, "label").into(),
                        price: format!("{} ₽", s(x, "rub")).into(),
                        note: if k == "forever" { "выгоднее всего".into() } else { "".into() },
                    });
                }
            }
        }
        if !can_add {
            let note = if nicks.is_empty() {
                "навсегда, без подписки. С PRO первый ник бесплатно".to_string()
            } else {
                let first = if n(&p, "nickPriceFirst") > 0 { n(&p, "nickPriceFirst") } else { 499 };
                let next = if n(&p, "nickPriceNext") > 0 { n(&p, "nickPriceNext") } else { 399 };
                format!("ещё один ник. Первый {first} ₽, каждый следующий {next} ₽")
            };
            plans.push(AccPlan { key: "nick".into(), label: "Цветной ник".into(), price: format!("{} ₽", s(&p, "nickPrice")).into(), note: note.into() });
        }
        let rows: Vec<AccNick> = nicks
            .iter()
            .map(|x| {
                let name = s(x, "nickname");
                let paid = x.get("is_paid").and_then(|v| v.as_bool()).unwrap_or(n(x, "is_paid") != 0);
                AccNick {
                    name: name.clone().into(),
                    note: if paid { "навсегда".into() } else { "по подписке PRO".into() },
                    key: crate::epic::key(&name).into(),
                    style: crate::epic::style(&name, dark),
                }
            })
            .collect();
        let rights = p.get("decor").cloned().unwrap_or(Value::Null);
        let label = |k: &str| match rights.get(k).and_then(|x| x.as_str()) {
            Some("bought") => "куплено навсегда",
            Some("pro") => "входит в PRO",
            _ => "не куплено",
        };
        let (rf, rb) = (label("frame").to_string(), label("banner").to_string());
        let has_f = rights.get("frame").map(|x| !x.is_null() && x != &Value::Bool(false)).unwrap_or(false);
        let has_b = rights.get("banner").map(|x| !x.is_null() && x != &Value::Bool(false)).unwrap_or(false);
        let prices = p.get("decorPrices").cloned().unwrap_or(Value::Null);
        let pf = if n(&prices, "frame") > 0 { n(&prices, "frame") } else { 149 };
        let pb = if n(&prices, "banner") > 0 { n(&prices, "banner") } else { 199 };
        let count = if nicks.is_empty() { "нет".to_string() } else { nicks.len().to_string() };
        let add_note = if credits > 0 { "Ник оплачен." } else { "С PRO первый ник бесплатно." };
        self.st(|st| {
            st.set_acc_pro(pro.into());
            st.set_acc_nick_count(count.into());
            st.set_acc_plans(model(plans));
            st.set_acc_can_add(can_add);
            st.set_acc_add_title(format!("Добавить ник {me}").into());
            st.set_acc_add_note(format!("{add_note} Собери стиль в генераторе, скопируй ключ и вставь сюда.").into());
            st.set_acc_nicks(model(rows));
            st.set_acc_has_frame(has_f);
            st.set_acc_has_banner(has_b);
            st.set_acc_r_frame(rf.into());
            st.set_acc_r_banner(rb.into());
            st.set_acc_price_frame(format!("{pf} ₽").into());
            st.set_acc_price_banner(format!("{pb} ₽").into());
        });
    }

    pub fn acc_tg_login(&mut self) {
        self.ext.acc_login_gen += 1;
        let gen = self.ext.acc_login_gen;
        LOGIN_GEN.store(gen, Ordering::SeqCst);
        self.st(|st| st.set_acc_busy(true));
        self.acc_msg("Открываю бота…", false);
        std::thread::spawn(move || {
            let s0 = api("/tg/start", json!({}));
            if !ok(&s0) {
                let e = err(&s0);
                post(move |u| u.acc_login_fail(gen, e));
                return;
            }
            let link = s(&s0, "link");
            let token = s(&s0, "token");
            crate::sys::open_external(&link);
            post(|u| u.acc_msg("Нажми Start в боте. Если бот попросит подписаться на канал, подпишись и нажми «Войти через Telegram» ещё раз.", false));
            let deadline = Instant::now() + Duration::from_secs(300);
            while Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(2500));
                let p = api("/tg/poll", json!({ "token": token }));
                if ok(&p) && p.get("session").and_then(|x| x.as_str()).map(|x| !x.is_empty()).unwrap_or(false) {
                    let sess = s(&p, "session");
                    let user = p.get("user").cloned().unwrap_or(Value::Null);
                    post(move |u| u.acc_tg_ok(gen, sess, user));
                    return;
                }
                if !ok(&p) && err(&p) != "NOT_YET" {
                    let e = err(&p);
                    post(move |u| u.acc_login_fail(gen, e));
                    return;
                }
                if LOGIN_GEN.load(Ordering::SeqCst) != gen {
                    return;
                }
            }
            post(move |u| u.acc_login_fail(gen, "LOGIN_TIMEOUT".into()));
        });
    }

    fn acc_login_fail(&mut self, gen: u64, e: String) {
        if gen != self.ext.acc_login_gen {
            return;
        }
        self.st(|st| st.set_acc_busy(false));
        self.acc_msg(&err_text(&e), true);
    }

    fn acc_tg_ok(&mut self, gen: u64, sess: String, user: Value) {
        if gen != self.ext.acc_login_gen {
            return;
        }
        let un = s(&user, "username");
        let label = if !un.is_empty() { format!("@{un}") } else if !s(&user, "name").is_empty() { s(&user, "name") } else { "подключён".into() };
        {
            let mut c = self.cfg.lock().unwrap();
            c.fpt_tg = (sess, label.clone());
            c.save();
        }
        self.st(|st| {
            st.set_acc_busy(false);
            st.set_acc_tg(true);
            st.set_acc_tg_name(label.into());
        });
        self.acc_msg("Telegram подключён.", false);
        self.toast("Telegram подключён");
        self.acc_reload();
    }

    pub fn acc_tg_logout(&mut self) {
        let sess = self.tg_session();
        if !sess.is_empty() {
            std::thread::spawn(move || {
                api("/tg/logout", json!({ "session": sess }));
            });
        }
        self.acc_forget_tg();
        self.acc_msg("", false);
    }

    pub fn acc_buy(&mut self, product: String) {
        let title = format!("{}: чем оплатить?", product_name(&product));
        self.st(|st| {
            st.set_acc_pay_product(product.into());
            st.set_acc_pay_step(1);
            st.set_acc_pay_title(title.into());
            st.set_acc_pay_state("".into());
        });
    }

    pub fn acc_pay(&mut self, method: String) {
        let sess = self.tg_session();
        let product = self.st(|st| st.get_acc_pay_product().to_string()).unwrap_or_default();
        if sess.is_empty() || product.is_empty() {
            return;
        }
        self.st(|st| {
            st.set_acc_busy(true);
            st.set_acc_pay_state("Создаю счёт…".into());
        });
        std::thread::spawn(move || {
            let r = job(&sess, "pay", json!({ "product": product, "method": method }));
            post(move |u| u.acc_invoice(product, r));
        });
    }

    fn acc_invoice(&mut self, product: String, r: Result<Value, String>) {
        self.st(|st| st.set_acc_busy(false));
        let inv = match r {
            Ok(v) => v,
            Err(e) => {
                self.st(|st| st.set_acc_pay_state("".into()));
                self.acc_msg(&err_text(&e), true);
                return;
            }
        };
        let url = s(&inv, "payUrl");
        let tid = s(&inv, "tid");
        if !url.is_empty() {
            crate::sys::open_external(&url);
        }
        let title = format!("{}: {} ₽", product_name(&product), s(&inv, "amount"));
        let state = format!("Оплата открылась в браузере. Счёт действует {}. Как только оплата пройдёт, всё включится само.", s(&inv, "expiresIn"));
        self.st(|st| {
            st.set_acc_pay_step(2);
            st.set_acc_pay_title(title.into());
            st.set_acc_pay_state(state.into());
        });
        self.ext.acc_pay = Some((tid, product));
        self.ext.acc_pay_gen += 1;
        let gen = self.ext.acc_pay_gen;
        PAY_GEN.store(gen, Ordering::SeqCst);
        // сами проверяем каждые 20 секунд, 20 минут
        std::thread::spawn(move || {
            for _ in 0..60 {
                std::thread::sleep(Duration::from_secs(20));
                if PAY_GEN.load(Ordering::SeqCst) != gen {
                    return;
                }
                post(move |u| {
                    if u.ext.acc_pay_gen == gen && u.ext.acc_pay.is_some() {
                        u.acc_pay_check_inner(false);
                    }
                });
            }
        });
    }

    pub fn acc_pay_check(&mut self) {
        self.acc_pay_check_inner(true);
    }

    fn acc_pay_check_inner(&mut self, manual: bool) {
        let Some((tid, product)) = self.ext.acc_pay.clone() else { return };
        let sess = self.tg_session();
        if manual {
            self.st(|st| st.set_acc_pay_state("Проверяю оплату…".into()));
        }
        std::thread::spawn(move || {
            let r = job(&sess, "check", json!({ "tid": tid }));
            post(move |u| u.acc_checked(manual, product, r));
        });
    }

    fn acc_checked(&mut self, manual: bool, product: String, r: Result<Value, String>) {
        match r {
            Ok(v) => match s(&v, "status").as_str() {
                "paid" => {
                    self.ext.acc_pay = None;
                    PAY_GEN.fetch_add(1, Ordering::SeqCst);
                    let text = match product.as_str() {
                        "nick" => "Оплата прошла. Вставь ключ стиля ниже.",
                        "frame" | "banner" => "Оплата прошла. Выбери украшение ниже и нажми «Сохранить украшения».",
                        _ => "Оплата прошла. Ключ PRO пришёл в бота @FPToolsBot.",
                    };
                    self.st(|st| {
                        st.set_acc_pay_step(0);
                        st.set_acc_pay_product("".into());
                    });
                    self.acc_msg(text, false);
                    self.toast(text);
                    self.acc_reload();
                }
                "canceled" => self.st(|st| st.set_acc_pay_state("Платёж отменён. Создай новый счёт.".into())).unwrap_or(()),
                _ => self.st(|st| st.set_acc_pay_state("Оплата пока не пришла. Проверяю каждые 20 секунд.".into())).unwrap_or(()),
            },
            Err(e) => {
                if manual {
                    self.acc_msg(&err_text(&e), true);
                }
            }
        }
    }

    pub fn acc_add_nick(&mut self, key: String) {
        let key = key.trim().to_string();
        if !key.starts_with("FPT-STYLE-") || crate::epic::decode_style(&key).is_none() {
            self.acc_msg(&err_text("BAD_KEY"), true);
            return;
        }
        self.acc_with_session(After::Nick(key));
    }

    pub fn acc_style(&mut self, idx: i32, key: String) {
        let key = key.trim().to_string();
        if !key.starts_with("FPT-STYLE-") || crate::epic::decode_style(&key).is_none() {
            self.acc_msg(&err_text("BAD_KEY"), true);
            return;
        }
        let name = self
            .ext
            .acc
            .as_ref()
            .and_then(|p| p.get("nicks").and_then(|x| x.as_array()).and_then(|a| a.get(idx.max(0) as usize).cloned()))
            .map(|x| s(&x, "nickname"))
            .unwrap_or_default();
        if name.is_empty() {
            return;
        }
        let sess = self.tg_session();
        self.st(|st| st.set_acc_busy(true));
        self.acc_msg("Меняю стиль…", false);
        std::thread::spawn(move || {
            let r = job(&sess, "style", json!({ "nickname": name, "key": key }));
            post(move |u| {
                u.st(|st| st.set_acc_busy(false));
                match r {
                    Ok(_) => {
                        crate::epic::set_local(&name, &key);
                        u.epic_loaded();
                        u.acc_render();
                        u.acc_msg(&format!("Стиль ника {name} обновлён. У тебя уже видно, у остальных поменяется в течение пары минут."), false);
                    }
                    Err(e) => u.acc_msg(&err_text(&e), true),
                }
            });
        });
    }

    pub fn acc_pick(&mut self, kind: i32, id: i32) {
        self.st(|st| {
            if kind == 0 {
                st.set_acc_df(id);
            } else {
                st.set_acc_db(id);
            }
        });
    }

    pub fn acc_decor_save(&mut self) {
        let (f, b, hf, hb) = self.st(|st| (st.get_acc_df(), st.get_acc_db(), st.get_acc_has_frame(), st.get_acc_has_banner())).unwrap_or((0, 0, false, false));
        if f > 0 && !hf {
            self.decor_msg("Рамку сначала нужно купить или взять PRO.", true);
            return;
        }
        if b > 0 && !hb {
            self.decor_msg("Баннер сначала нужно купить или взять PRO.", true);
            return;
        }
        self.acc_with_session(After::Decor(f, b));
    }

    /// Подтверждённый аккаунт FunPay нужен для ника и украшений: проверяем сохранённую сессию, иначе временный лот.
    fn acc_with_session(&mut self, after: After) {
        let uid = self.me.id.clone();
        if uid.is_empty() {
            self.acc_msg("Сначала войди в аккаунт FunPay.", true);
            return;
        }
        self.ext.acc_after = Some(after);
        self.st(|st| st.set_acc_busy(true));
        let saved = {
            let c = self.cfg.lock().unwrap();
            if c.promo_session.0 == uid { c.promo_session.1.clone() } else { String::new() }
        };
        self.acc_progress("Проверяю аккаунт FunPay…");
        std::thread::spawn(move || {
            if !saved.is_empty() {
                let st = api("/status", json!({ "session": saved }));
                if ok(&st) {
                    post(move |u| u.acc_session_done(Ok(saved)));
                    return;
                }
            }
            post(move |u| u.send(Cmd::PromoLink { fpid: uid }));
        });
    }

    pub fn acc_progress(&mut self, t: &str) {
        let decor = matches!(self.ext.acc_after, Some(After::Decor(..)));
        if decor {
            self.decor_msg(t, false);
        } else {
            self.acc_msg(t, false);
        }
    }

    pub fn acc_session_done(&mut self, r: Result<String, String>) {
        let Some(after) = self.ext.acc_after.take() else {
            self.st(|st| st.set_acc_busy(false));
            return;
        };
        let decor = matches!(after, After::Decor(..));
        let token = match r {
            Ok(t) => t,
            Err(e) => {
                self.st(|st| st.set_acc_busy(false));
                let text = err_text(&e);
                if decor { self.decor_msg(&text, true) } else { self.acc_msg(&text, true) }
                return;
            }
        };
        let uid = self.me.id.clone();
        {
            let mut c = self.cfg.lock().unwrap();
            c.promo_session = (uid.clone(), token.clone());
            c.save();
        }
        let sess = self.tg_session();
        match after {
            After::Nick(key) => {
                self.acc_msg("Ставлю ник…", false);
                std::thread::spawn(move || {
                    let r = job(&sess, "nick", json!({ "fpSession": token, "key": key }));
                    post(move |u| u.acc_nick_done(key, r));
                });
            }
            After::Decor(f, b) => {
                self.decor_msg("Сохраняю…", false);
                std::thread::spawn(move || {
                    let r = job(&sess, "decor", json!({ "fpSession": token, "frame": f, "banner": b }));
                    post(move |u| u.acc_decor_done(uid, f, b, r));
                });
            }
        }
    }

    fn forget_promo_if(&mut self, e: &str) {
        if e == "NO_SESSION" {
            let mut c = self.cfg.lock().unwrap();
            c.promo_session = (String::new(), String::new());
            c.save();
        }
    }

    fn acc_nick_done(&mut self, key: String, r: Result<Value, String>) {
        self.st(|st| st.set_acc_busy(false));
        match r {
            Ok(v) => {
                let name = s(&v, "nickname");
                if !name.is_empty() {
                    crate::epic::set_local(&name, &key);
                    self.epic_loaded();
                }
                self.acc_msg(&format!("Готово! У тебя ник {name} уже светится, у остальных в течение пары минут."), false);
                self.acc_reload();
            }
            Err(e) => {
                self.forget_promo_if(&e);
                self.acc_msg(&err_text(&e), true);
            }
        }
    }

    fn acc_decor_done(&mut self, uid: String, f: i32, b: i32, r: Result<Value, String>) {
        self.st(|st| st.set_acc_busy(false));
        match r {
            Ok(_) => {
                decor::set_local(&uid, f, b);
                self.decor_loaded();
                self.decor_msg("Готово! У тебя уже видно, у остальных обновится в течение пары минут.", false);
                // через полторы минуты подтягиваем свежий decor.json
                std::thread::spawn(|| {
                    std::thread::sleep(Duration::from_secs(90));
                    decor::load(|| post(|u| u.decor_loaded()));
                });
            }
            Err(e) => {
                self.forget_promo_if(&e);
                self.decor_msg(&err_text(&e), true);
            }
        }
    }
}

/// Подтверждение аккаунта FunPay временным лотом (в потоке воркера, где живёт клиент FunPay).
pub fn link_start(fpid: &str) -> Result<String, String> {
    let r = api("/link/start", json!({ "fpid": fpid }));
    if !ok(&r) {
        return Err(err(&r));
    }
    let code = s(&r, "code");
    if code.is_empty() { Err("LINK_START".into()) } else { Ok(code) }
}

/// Ждём, пока сервер увидит лот (до минуты). Ok(сессия) или Err(код).
pub fn link_wait(fpid: &str, offer: &str) -> Result<String, String> {
    let deadline = Instant::now() + Duration::from_secs(60);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_secs(3));
        let offer_num: i64 = offer.parse().unwrap_or(0);
        let c = api("/link/confirm", json!({ "fpid": fpid, "offerId": offer_num }));
        if ok(&c) {
            let sess = s(&c, "session");
            if !sess.is_empty() {
                return Ok(sess);
            }
        }
        let e = err(&c);
        if !ok(&c) && e != "NOT_YET" && e != "UNKNOWN" {
            return Err(e);
        }
    }
    Err("VERIFY_TIMEOUT".into())
}

