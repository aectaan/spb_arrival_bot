use chrono::Local;
use lazy_static::lazy_static;
use std::{collections::HashMap, error::Error, time::Duration};
use teloxide::{
    dispatching::{
        dialogue::{self, serializer::Json, ErasedStorage, SqliteStorage, Storage},
        UpdateHandler,
    },
    prelude::*,
    types::{InlineKeyboardButton, InlineKeyboardMarkup, MenuButton, MessageId},
    utils::command::BotCommands,
};
use tokio::sync::Mutex;
use tokio::task::JoinHandle;

use crate::gtfs::{self, RouteId, StopId, Vehicle};
use crate::saved_routes_db::SavedRoutesDb;
use crate::STATIC_FEED;

lazy_static! {
    static ref POLL_TASKS: Mutex<HashMap<ChatId, JoinHandle<HandlerResult>>> =
        Mutex::new(HashMap::new());
}

type MyDialogue = Dialogue<State, ErasedStorage<State>>;
type HandlerResult = Result<(), Box<dyn std::error::Error + Send + Sync>>;
type MyStorage = std::sync::Arc<ErasedStorage<State>>;

pub type SavedRouteName = String;
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct SavedRouteData {
    route_id: RouteId,
    stop_id: StopId,
    direction: String,
    leeway: u64,
}

pub type SavedRoutes = HashMap<SavedRouteName, SavedRouteData>;

#[derive(BotCommands, Clone)]
#[command(
    rename_rule = "lowercase",
    description = "These commands are supported:"
)]
enum Command {
    #[command(description = "Начать заново")]
    Start,
}

#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
enum State {
    #[default]
    BotStart,
    Start {
        bot_msg: MessageId,
    },
    NewOrSaved,
    DeleteRecord,
    RouteNumber {
        bot_msg: MessageId,
    },
    RouteDirection,
    RouteStop {
        route_id: RouteId,
    },
    RequestLeewayTime {
        route_id: RouteId,
        direction: String,
    },
    ReceiveLeewayTime {
        route_id: RouteId,
        stop_id: StopId,
        direction: String,
        bot_msg: MessageId,
    },
    SaveQuery {
        route_id: RouteId,
        stop_id: StopId,
        direction: String,
        leeway: u64,
    },
    SaveQueryName {
        route_id: RouteId,
        stop_id: StopId,
        direction: String,
        leeway: u64,
        bot_msg: MessageId,
    },
    Search {
        bot_msg: MessageId,
    },
}

pub async fn bot() {
    let bot = Bot::from_env();

    let storage: MyStorage = SqliteStorage::open("db/dialogues.sqlite", Json)
        .await
        .unwrap()
        .erase();

    Dispatcher::builder(bot, schema())
        .dependencies(dptree::deps![storage])
        .enable_ctrlc_handler()
        .build()
        .dispatch()
        .await;
}

fn schema() -> UpdateHandler<Box<dyn Error + Send + Sync + 'static>> {
    use dptree::case;

    let command_handler =
        teloxide::filter_command::<Command, _>().branch(case![Command::Start].endpoint(bot_start));

    let message_handler = Update::filter_message()
        .branch(command_handler)
        .branch(case![State::BotStart].endpoint(bot_start))
        .branch(case![State::RouteNumber { bot_msg }].endpoint(route_number))
        .branch(
            case![State::ReceiveLeewayTime {
                route_id,
                stop_id,
                direction,
                bot_msg
            }]
            .endpoint(receive_leeway_time),
        )
        .branch(
            case![State::SaveQueryName {
                route_id,
                stop_id,
                direction,
                leeway,
                bot_msg,
            }]
            .endpoint(save_query_name),
        )
        .branch(case![State::Start { bot_msg }].endpoint(delete_unexpected))
        .branch(case![State::NewOrSaved].endpoint(delete_unexpected))
        .branch(case![State::DeleteRecord].endpoint(delete_unexpected))
        .branch(case![State::RouteDirection].endpoint(delete_unexpected))
        .branch(case![State::RouteStop { route_id }].endpoint(delete_unexpected))
        .branch(
            case![State::RequestLeewayTime {
                route_id,
                direction
            }]
            .endpoint(delete_unexpected),
        )
        .branch(
            case![State::SaveQuery {
                route_id,
                stop_id,
                direction,
                leeway
            }]
            .endpoint(delete_unexpected),
        )
        .branch(case![State::Search { bot_msg }].endpoint(delete_unexpected));

    let callback_query_handler = Update::filter_callback_query()
        .branch(case![State::Start { bot_msg }].endpoint(start))
        .branch(case![State::NewOrSaved].endpoint(new_or_saved))
        .branch(case![State::DeleteRecord].endpoint(delete_record))
        .branch(case![State::RouteDirection].endpoint(route_direction))
        .branch(case![State::RouteStop { route_id }].endpoint(route_stop))
        .branch(
            case![State::RequestLeewayTime {
                route_id,
                direction,
            }]
            .endpoint(request_leeway_time),
        )
        .branch(
            case![State::SaveQuery {
                route_id,
                stop_id,
                direction,
                leeway
            }]
            .endpoint(save_query),
        )
        .branch(case![State::Search { bot_msg }].endpoint(search));

    dialogue::enter::<Update, ErasedStorage<State>, State, _>()
        .branch(message_handler)
        .branch(callback_query_handler)
}

async fn bot_start(bot: Bot, dialogue: MyDialogue) -> HandlerResult {
    bot.set_my_commands(Command::bot_commands()).await?;
    bot.set_chat_menu_button()
        .menu_button(MenuButton::Commands)
        .chat_id(dialogue.chat_id())
        .await?;

    delete_all(bot.clone(), dialogue.clone()).await;

    let keys: Vec<Vec<InlineKeyboardButton>> = vec![vec![InlineKeyboardButton::callback(
        "Начать работу",
        "start",
    )]];
    let keyboard = InlineKeyboardMarkup::new(keys);

    bot.send_message(dialogue.chat_id(), "Нажмите кнопку и мы начнем!")
        .reply_markup(keyboard)
        .await?;

    dialogue.update(State::NewOrSaved).await?;
    Ok(())
}

async fn start(
    bot: Bot,
    dialogue: MyDialogue,
    bot_msg: MessageId,
    q: CallbackQuery,
) -> HandlerResult {
    bot.answer_callback_query(q.id).await?;

    let mut keys: Vec<Vec<InlineKeyboardButton>> = vec![vec![InlineKeyboardButton::callback(
        "Новый маршрут",
        "new_route",
    )]];

    let saved_routes = dialogue.chat_id().get_saved_routes()?;

    for key in saved_routes.keys() {
        keys.push(vec![InlineKeyboardButton::callback(key, key)]);
    }

    if !saved_routes.is_empty() {
        keys.push(vec![InlineKeyboardButton::callback(
            "Удалить сохраненный",
            "delete",
        )]);
    }
    let keyboard = InlineKeyboardMarkup::new(keys);

    bot.edit_message_text(dialogue.chat_id(), bot_msg, "🚗Куда едем?🚙")
        .reply_markup(keyboard)
        .await?;

    dialogue.update(State::NewOrSaved).await?;
    Ok(())
}

async fn new_or_saved(bot: Bot, dialogue: MyDialogue, q: CallbackQuery) -> HandlerResult {
    bot.answer_callback_query(q.id).await?;

    let bot_msg = q.message.unwrap().id;

    if let Some(select) = q.data {
        let saved_routes = dialogue.chat_id().get_saved_routes()?;
        if select == "delete" {
            let mut keys: Vec<Vec<InlineKeyboardButton>> = vec![];
            for name in saved_routes.keys() {
                keys.push(vec![InlineKeyboardButton::callback(name, name)]);
            }
            let keyboard = InlineKeyboardMarkup::new(keys);

            bot.edit_message_text(dialogue.chat_id(), bot_msg, "Что удаляем?")
                .reply_markup(keyboard)
                .await?;

            dialogue.update(State::DeleteRecord).await?;
        } else if let Some(route_data) = saved_routes.get(&select) {
            let keys: Vec<Vec<InlineKeyboardButton>> = vec![vec![InlineKeyboardButton::callback(
                "🚫Отменить поиск🚫",
                String::from("cancel"),
            )]];
            let keyboard = InlineKeyboardMarkup::new(keys);

            bot.edit_message_text(
                dialogue.chat_id(),
                bot_msg,
                "✅Готово! Я пришлю напоминание перед выходом",
            )
            .reply_markup(keyboard)
            .await?;

            let polling_handle = tokio::spawn(look_for_transport(
                bot,
                dialogue.clone(),
                (
                    route_data.route_id.clone(),
                    route_data.stop_id.clone(),
                    route_data.direction.clone(),
                    route_data.leeway as i64,
                    bot_msg,
                ),
            ));

            if let Some(task) = POLL_TASKS
                .lock()
                .await
                .insert(dialogue.chat_id(), polling_handle)
            {
                task.abort();
            }

            dialogue.update(State::Search { bot_msg }).await?;
        } else {
            bot.edit_message_text(
                dialogue.chat_id(),
                bot_msg,
                "🔢Введите номер маршрута, например 1Кр🔢",
            )
            .await?;

            dialogue.update(State::RouteNumber { bot_msg }).await?;
        }
    }

    Ok(())
}

async fn delete_record(bot: Bot, dialogue: MyDialogue, q: CallbackQuery) -> HandlerResult {
    bot.answer_callback_query(q.id).await?;

    if let Some(name) = q.data {
        dialogue.chat_id().remove_route_from_saved(&name)?;

        let bot_msg = q.message.unwrap().id;

        let keys: Vec<Vec<InlineKeyboardButton>> = vec![vec![InlineKeyboardButton::callback(
            "Новый поиск",
            String::from("new"),
        )]];
        let keyboard = InlineKeyboardMarkup::new(keys);

        bot.edit_message_text(dialogue.chat_id(), bot_msg, "Маршрут удален из сохраненных")
            .reply_markup(keyboard)
            .await?;

        dialogue.update(State::Start { bot_msg }).await?;
    }
    Ok(())
}

async fn route_number(
    bot: Bot,
    dialogue: MyDialogue,
    bot_msg: MessageId,
    msg: Message,
) -> HandlerResult {
    if let Some(number) = msg.text() {
        let mut keys: Vec<Vec<InlineKeyboardButton>> = vec![];
        {
            let feed = STATIC_FEED.read().await;
            let number = number.to_uppercase();

            if let Some(bus) = feed.routes.bus.get(&number) {
                keys.push(vec![InlineKeyboardButton::callback(
                    format!("{} {}", Vehicle::Bus, number),
                    bus.clone().id,
                )]);
            }
            if let Some(trolley) = feed.routes.trolley.get(&number) {
                keys.push(vec![InlineKeyboardButton::callback(
                    format!("{} {}", Vehicle::Trolley, number),
                    trolley.clone().id,
                )]);
            }
            if let Some(tram) = feed.routes.tram.get(&number) {
                keys.push(vec![InlineKeyboardButton::callback(
                    format!("{} {}", Vehicle::Tram, number),
                    tram.clone().id,
                )]);
            }
        }

        if !keys.is_empty() {
            let keyboard = InlineKeyboardMarkup::new(keys);

            bot.edit_message_text(dialogue.chat_id(), bot_msg, "🔍 Вот что удалось найти:")
                .reply_markup(keyboard)
                .await?;
            dialogue.update(State::RouteDirection).await?;
        } else {
            bot.edit_message_text(
                dialogue.chat_id(),
                bot_msg,
                "🤖 К сожалению, я ничего не нашел. Попробуйте ввести другой номер.",
            )
            .await?;
            dialogue.update(State::RouteNumber { bot_msg }).await?;
        }
    }
    bot.delete_message(dialogue.chat_id(), msg.id).await?;
    Ok(())
}

async fn route_direction(bot: Bot, dialogue: MyDialogue, q: CallbackQuery) -> HandlerResult {
    bot.answer_callback_query(q.id.clone()).await?;

    let bot_msg = q.message.unwrap().id;

    if let Some(route_id) = q.data {
        let route_name = gtfs::route_name(&route_id).await?;

        let keys: Vec<Vec<InlineKeyboardButton>> = vec![vec![
            InlineKeyboardButton::callback("➡️Туда➡️", String::from("0")),
            InlineKeyboardButton::callback("⬅️Обратно⬅️", String::from("1")),
        ]];
        let keyboard = InlineKeyboardMarkup::new(keys);

        bot.edit_message_text(
            dialogue.chat_id(),
            bot_msg,
            format!("{route_name}\r\nВыберите направление:"),
        )
        .reply_markup(keyboard)
        .await?;

        dialogue.update(State::RouteStop { route_id }).await?;
    }
    Ok(())
}

async fn route_stop(
    bot: Bot,
    dialogue: MyDialogue,
    route_id: RouteId,
    q: CallbackQuery,
) -> HandlerResult {
    bot.answer_callback_query(q.id).await?;

    let msg = q.message.unwrap().id;

    if let Some(mut direction) = q.data {
        let mut stops = gtfs::stops_on_route(&route_id, &direction).await;
        // There are circular routes that serve only in one direction (Bus 261 for example).
        // Nevertheless, they have trip IDs for return route (that doesn't exist) and that trip IDs are not presented in `stop_times.txt`
        // that means we will always fail when trying to get corresponding stops. So this weird workaround designed to handle this shit correctly.
        if stops.is_err() {
            direction = if direction == "0" {
                "1".to_string()
            } else {
                "0".to_string()
            };
            stops = gtfs::stops_on_route(&route_id, &direction).await;
        }

        let mut keys: Vec<Vec<InlineKeyboardButton>> = vec![];

        for id in stops.unwrap() {
            let name = gtfs::stop_name(&id).await?;
            keys.push(vec![InlineKeyboardButton::callback(name, id)]);
        }
        let keyboard = InlineKeyboardMarkup::new(keys);

        bot.edit_message_text(dialogue.chat_id(), msg, "🚏Выберите остановку:")
            .reply_markup(keyboard)
            .await?;

        dialogue
            .update(State::RequestLeewayTime {
                route_id,
                direction,
            })
            .await?;
    }
    Ok(())
}

async fn request_leeway_time(
    bot: Bot,
    dialogue: MyDialogue,
    (route_id, direction): (RouteId, String),
    q: CallbackQuery,
) -> HandlerResult {
    bot.answer_callback_query(q.id).await?;

    if let Some(stop_id) = q.data {
        let bot_msg = q.message.unwrap().id;

        bot.edit_message_text(
            dialogue.chat_id(),
            bot_msg,
            "🕗Сколько минут идти до остановки?",
        )
        .await?;

        dialogue
            .update(State::ReceiveLeewayTime {
                route_id,
                stop_id,
                direction,
                bot_msg,
            })
            .await?;
    }
    Ok(())
}

async fn receive_leeway_time(
    bot: Bot,
    dialogue: MyDialogue,
    (route_id, stop_id, direction, bot_msg): (RouteId, StopId, String, MessageId),
    msg: Message,
) -> HandlerResult {
    if let Some(leeway) = msg.text() {
        if let Ok(leeway_minutes) = leeway.parse::<u64>() {
            let keys: Vec<Vec<InlineKeyboardButton>> = vec![vec![
                InlineKeyboardButton::callback("Да", String::from("yes")),
                InlineKeyboardButton::callback("нет", String::from("no")),
            ]];

            let keyboard = InlineKeyboardMarkup::new(keys);

            bot.edit_message_text(dialogue.chat_id(), bot_msg, "💾Cохранить маршрут?")
                .reply_markup(keyboard)
                .await?;

            dialogue
                .update(State::SaveQuery {
                    route_id,
                    stop_id,
                    direction,
                    leeway: leeway_minutes,
                })
                .await?
        } else {
            bot.edit_message_text(
                dialogue.chat_id(),
                bot_msg,
                "🤖Мне не удалось распознать запрос. Пожалуйста, введите число",
            )
            .await?;
            dialogue
                .update(State::ReceiveLeewayTime {
                    route_id,
                    stop_id,
                    direction,
                    bot_msg,
                })
                .await?;
        }
    }
    bot.delete_message(dialogue.chat_id(), msg.id).await?;
    Ok(())
}

async fn save_query(
    bot: Bot,
    dialogue: MyDialogue,
    (route_id, stop_id, direction, leeway): (RouteId, StopId, String, u64),
    q: CallbackQuery,
) -> HandlerResult {
    bot.answer_callback_query(q.id).await?;

    if let Some(save) = q.data {
        let bot_msg = q.message.unwrap().id;

        if save.as_str().eq("yes") {
            bot.edit_message_text(dialogue.chat_id(), bot_msg, "🔖Введите имя для маршрута:")
                .await?;
            dialogue
                .update(State::SaveQueryName {
                    route_id,
                    stop_id,
                    direction,
                    leeway,
                    bot_msg,
                })
                .await?;
        } else {
            let keys: Vec<Vec<InlineKeyboardButton>> = vec![vec![InlineKeyboardButton::callback(
                "🚫Отменить поиск🚫",
                String::from("cancel"),
            )]];
            let keyboard = InlineKeyboardMarkup::new(keys);

            bot.edit_message_text(
                dialogue.chat_id(),
                bot_msg,
                "✅Готово! Я пришлю напоминание перед выходом",
            )
            .reply_markup(keyboard)
            .await?;

            let polling_handle = tokio::spawn(look_for_transport(
                bot,
                dialogue.clone(),
                (route_id, stop_id, direction, leeway as i64, bot_msg),
            ));

            if let Some(task) = POLL_TASKS
                .lock()
                .await
                .insert(dialogue.chat_id(), polling_handle)
            {
                task.abort();
            }

            dialogue.update(State::Search { bot_msg }).await?;
        }
    }

    Ok(())
}

async fn save_query_name(
    bot: Bot,
    dialogue: MyDialogue,
    (route_id, stop_id, direction, leeway, bot_msg): (RouteId, StopId, String, u64, MessageId),
    msg: Message,
) -> HandlerResult {
    if let Some(name) = msg.text() {
        dialogue.chat_id().add_route_to_saved(
            name.to_string(),
            SavedRouteData {
                route_id: route_id.clone(),
                stop_id: stop_id.clone(),
                direction: direction.clone(),
                leeway,
            },
        )?;

        let keys: Vec<Vec<InlineKeyboardButton>> = vec![vec![InlineKeyboardButton::callback(
            "🚫Отменить поиск🚫",
            String::from("cancel"),
        )]];
        let keyboard = InlineKeyboardMarkup::new(keys);

        bot.edit_message_text(
            dialogue.chat_id(),
            bot_msg,
            "✅Готово! Я пришлю напоминание перед выходом",
        )
        .reply_markup(keyboard)
        .await?;

        let polling_handle = tokio::spawn(look_for_transport(
            bot.clone(),
            dialogue.clone(),
            (route_id, stop_id, direction, leeway as i64, bot_msg),
        ));

        if let Some(task) = POLL_TASKS
            .lock()
            .await
            .insert(dialogue.chat_id(), polling_handle)
        {
            task.abort();
        }

        dialogue.update(State::Search { bot_msg }).await?;
    }
    bot.delete_message(dialogue.chat_id(), msg.id).await?;
    Ok(())
}

async fn search(
    bot: Bot,
    dialogue: MyDialogue,
    bot_msg: MessageId,
    q: CallbackQuery,
) -> HandlerResult {
    bot.answer_callback_query(q.id).await?;

    if let Some(str) = q.data {
        if str == "cancel" {
            if let Some(jh) = POLL_TASKS.lock().await.remove(&dialogue.chat_id()) {
                jh.abort();

                let keys: Vec<Vec<InlineKeyboardButton>> =
                    vec![vec![InlineKeyboardButton::callback(
                        "🆕Новый поиск🆕",
                        String::from("new"),
                    )]];
                let keyboard = InlineKeyboardMarkup::new(keys);

                bot.edit_message_text(dialogue.chat_id(), bot_msg, "⛔️Поиск отменен⛔️")
                    .reply_markup(keyboard)
                    .await?;

                dialogue.update(State::Start { bot_msg }).await?;
            }
        }
    }
    Ok(())
}

async fn look_for_transport(
    bot: Bot,
    dialogue: MyDialogue,
    (route_id, stop_id, direction, leeway, bot_msg): (RouteId, StopId, String, i64, MessageId),
) -> HandlerResult {
    let timetable = gtfs::arrival_timetable(&route_id, &direction, &stop_id).await?;

    loop {
        if let Ok(forecast) = gtfs::arrival_forecast(&route_id, &stop_id).await {
            let waiting_list = forecast
                .iter()
                .filter(|&&x| x - (leeway * 60) > 0)
                .collect::<Vec<&i64>>();
            log::warn!(
                "Chat ID {} waiting time for route {} at stop {} is {:?}",
                dialogue.chat_id().0,
                route_id,
                stop_id,
                waiting_list
            );

            if waiting_list.is_empty() {
                let time = Local::now().timestamp() + (leeway * 60);
                let next_on_timetable = timetable
                    .iter()
                    .filter_map(|t| if t - time > 0 { Some(t - time) } else { None })
                    .collect::<Vec<i64>>();
                log::warn!(
                    "Chat ID {} waiting time by timetable for route {} at stop {} is {:?}",
                    dialogue.chat_id().0,
                    route_id,
                    stop_id,
                    next_on_timetable
                );
                if next_on_timetable.iter().any(|&t| t < 60) {
                    log::warn!("Yielded by timetable");

                    let keys: Vec<Vec<InlineKeyboardButton>> =
                        vec![vec![InlineKeyboardButton::callback(
                            "🆕Новый поиск🆕",
                            String::from("new"),
                        )]];

                    let keyboard = InlineKeyboardMarkup::new(keys);

                    bot.delete_message(dialogue.chat_id(), bot_msg).await?;
                    let bot_msg = bot.send_message(dialogue.chat_id(), "⏰Я не нашел актуальных данных, но если верить расписанию, пора выходить!⏰").reply_markup(keyboard).await?.id;

                    dialogue.update(State::Start { bot_msg }).await?;

                    return Ok(());
                }
            } else {
                for time in waiting_list {
                    if let Some(time_left) = time.checked_sub(leeway * 60) {
                        if time_left < 60 {
                            log::warn!("Signalled by actual data");
                            let keys: Vec<Vec<InlineKeyboardButton>> =
                                vec![vec![InlineKeyboardButton::callback(
                                    "🆕Новый поиск🆕",
                                    String::from("new"),
                                )]];

                            let keyboard = InlineKeyboardMarkup::new(keys);

                            bot.delete_message(dialogue.chat_id(), bot_msg).await?;
                            let bot_msg = bot
                                .send_message(dialogue.chat_id(), "⏰Пора выходить!⏰")
                                .reply_markup(keyboard)
                                .await?
                                .id;

                            dialogue.update(State::Start { bot_msg }).await?;
                            return Ok(());
                        }
                    }
                }
            }
        }
        tokio::time::sleep(Duration::from_secs(5)).await; // we'll continue polling in case of nothing found
    }
}

async fn delete_unexpected(bot: Bot, dialogue: MyDialogue, msg: Message) -> HandlerResult {
    bot.delete_message(dialogue.chat_id(), msg.id).await?;
    Ok(())
}

async fn delete_all(bot: Bot, dialogue: MyDialogue) {
    let msg = bot
        .send_message(dialogue.chat_id(), "deleting")
        .await
        .unwrap();

    tokio::spawn(async move {
        for id in (0..=msg.id.0).rev() {
            let _ = bot.delete_message(dialogue.chat_id(), MessageId(id)).await;
        }
    });
}
