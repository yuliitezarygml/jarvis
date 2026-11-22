//! Таблицы для `open_target`: страницы `ms-settings:` с русскими синонимами, русские названия приложений,
//! известные папки.

use crate::win::KnownFolder;

/// (URI, синонимы). Первый синоним — название для ответа.
pub const SETTINGS: &[(&str, &[&str])] = &[
    ("ms-settings:bluetooth", &["Bluetooth и устройства", "bluetooth", "блютуз", "блютус", "блютус устройства", "устройства bluetooth"]),
    ("ms-settings:network-wifi", &["Wi-Fi", "wi fi", "wifi", "вайфай", "вай фай", "беспроводная сеть", "беспроводные сети"]),
    ("ms-settings:sound", &["Звук", "sound", "громкость", "аудио", "динамики", "звуковые устройства"]),
    ("ms-settings:display", &["Дисплей", "display", "экран", "монитор", "разрешение экрана", "яркость", "масштаб"]),
    ("ms-settings:appsfeatures", &["Приложения", "apps", "установленные приложения", "программы", "удаление программ", "приложения и возможности"]),
    ("ms-settings:defaultapps", &["Приложения по умолчанию", "default apps", "программы по умолчанию"]),
    ("ms-settings:startupapps", &["Автозагрузка", "startup apps", "автозапуск", "автозагрузка приложений"]),
    ("ms-settings:windowsupdate", &["Центр обновления Windows", "windows update", "обновления", "обновление", "обновление windows", "центр обновления"]),
    ("ms-settings:batterysaver", &["Батарея", "battery", "аккумулятор", "экономия заряда", "заряд"]),
    ("ms-settings:powersleep", &["Питание и спящий режим", "power", "питание", "электропитание", "сон", "спящий режим"]),
    ("ms-settings:mousetouchpad", &["Мышь", "mouse", "мышка", "курсор мыши", "колесо мыши"]),
    ("ms-settings:devices-touchpad", &["Сенсорная панель", "touchpad", "тачпад"]),
    ("ms-settings:typing", &["Ввод и клавиатура", "keyboard", "клавиатура", "ввод", "раскладка"]),
    ("ms-settings:printers", &["Принтеры и сканеры", "printers", "принтеры", "принтер", "сканеры", "сканер", "печать"]),
    ("ms-settings:network-status", &["Сеть и Интернет", "network", "сеть", "интернет", "сеть и интернет", "сетевые настройки"]),
    ("ms-settings:network-ethernet", &["Ethernet", "ethernet", "проводная сеть", "кабель", "локальная сеть"]),
    ("ms-settings:network-vpn", &["VPN", "vpn", "впн", "виртуальная частная сеть"]),
    ("ms-settings:network-proxy", &["Прокси", "proxy", "прокси", "прокси сервер"]),
    ("ms-settings:network-airplanemode", &["Режим «в самолёте»", "airplane mode", "режим полета", "режим в самолете", "авиарежим"]),
    ("ms-settings:network-mobilehotspot", &["Мобильный хот-спот", "hotspot", "хот спот", "точка доступа", "раздача интернета"]),
    ("ms-settings:dateandtime", &["Дата и время", "date and time", "дата и время", "время", "дата", "часовой пояс", "часы"]),
    ("ms-settings:regionlanguage", &["Язык и регион", "language", "язык", "регион", "язык и регион", "язык интерфейса"]),
    ("ms-settings:privacy", &["Конфиденциальность и защита", "privacy", "конфиденциальность", "приватность"]),
    ("ms-settings:privacy-webcam", &["Камера (разрешения)", "camera", "камера", "веб камера", "вебкамера", "доступ к камере"]),
    ("ms-settings:privacy-microphone", &["Микрофон (разрешения)", "microphone", "микрофон", "доступ к микрофону"]),
    ("ms-settings:privacy-location", &["Расположение", "location", "местоположение", "геолокация", "расположение"]),
    ("ms-settings:notifications", &["Уведомления", "notifications", "уведомления", "оповещения"]),
    ("ms-settings:quiethours", &["Не беспокоить", "focus assist", "не беспокоить", "фокусировка внимания", "фокусировка"]),
    ("ms-settings:storagesense", &["Хранилище", "storage", "хранилище", "память диска", "место на диске", "контроль памяти", "диск"]),
    ("ms-settings:recovery", &["Восстановление", "recovery", "восстановление", "сброс компьютера", "вернуть в исходное состояние"]),
    ("ms-settings:activation", &["Активация", "activation", "активация", "активация windows", "ключ продукта"]),
    ("ms-settings:developers", &["Для разработчиков", "developers", "для разработчиков", "режим разработчика"]),
    ("ms-settings:nightlight", &["Ночной свет", "night light", "ночной свет", "ночной режим", "теплые цвета"]),
    ("ms-settings:personalization", &["Персонализация", "personalization", "персонализация", "оформление"]),
    ("ms-settings:themes", &["Темы", "themes", "темы", "тема", "тема оформления"]),
    ("ms-settings:personalization-background", &["Фон", "background", "фон", "обои", "фон рабочего стола", "заставка рабочего стола"]),
    ("ms-settings:colors", &["Цвета", "colors", "цвета", "цвет", "темная тема", "светлая тема", "темный режим"]),
    ("ms-settings:lockscreen", &["Экран блокировки", "lock screen", "экран блокировки", "блокировка экрана"]),
    ("ms-settings:taskbar", &["Панель задач", "taskbar", "панель задач"]),
    ("ms-settings:personalization-start", &["Пуск", "start", "меню пуск", "пуск"]),
    ("ms-settings:yourinfo", &["Учётные записи", "accounts", "учетные записи", "учетная запись", "аккаунт", "ваши данные", "профиль"]),
    ("ms-settings:signinoptions", &["Варианты входа", "sign in options", "варианты входа", "пин код", "пароль входа", "windows hello"]),
    ("ms-settings:emailandaccounts", &["Почта и учётные записи", "email accounts", "почта и учетные записи", "учетные записи почты"]),
    ("ms-settings:about", &["О системе", "about", "о системе", "о компьютере", "характеристики", "сведения о системе", "имя компьютера"]),
    ("ms-settings:multitasking", &["Многозадачность", "multitasking", "многозадачность", "привязка окон"]),
    ("ms-settings:clipboard", &["Буфер обмена", "clipboard", "буфер обмена", "журнал буфера обмена"]),
    ("ms-settings:gaming-gamemode", &["Игровой режим", "game mode", "игровой режим", "игры"]),
    ("ms-settings:gaming-gamebar", &["Xbox Game Bar", "game bar", "игровая панель"]),
    ("ms-settings:easeofaccess", &["Специальные возможности", "accessibility", "специальные возможности", "доступность"]),
    ("ms-settings:backup", &["Резервное копирование", "backup", "резервное копирование", "архивация", "бэкап"]),
    ("ms-settings:troubleshoot", &["Устранение неполадок", "troubleshoot", "устранение неполадок", "неполадки"]),
    ("ms-settings:optionalfeatures", &["Дополнительные компоненты", "optional features", "дополнительные компоненты", "компоненты windows"]),
    ("ms-settings:usb", &["USB", "usb"]),
    ("ms-settings:autoplay", &["Автозапуск носителей", "autoplay", "автозапуск носителей", "автозапуск флешки"]),
    ("ms-settings:mobile-devices", &["Мобильные устройства", "mobile devices", "телефон", "мобильные устройства", "связь с телефоном"]),
    ("windowsdefender:", &["Безопасность Windows", "windows security", "безопасность windows", "защитник", "антивирус", "defender", "защита от вирусов"]),
    ("ms-settings:", &["Параметры Windows", "settings", "параметры", "настройки", "параметры windows"]),
];

/// (русские и разговорные названия, канонические имена для поиска в «Пуске»/App Paths/PATH).
pub const APP_ALIASES: &[(&[&str], &[&str])] = &[
    (&["блокнот", "notepad", "текстовый редактор"], &["notepad", "блокнот"]),
    (&["калькулятор", "calculator"], &["calc", "калькулятор", "calculator"]),
    (&["проводник", "explorer", "файлы"], &["explorer", "проводник"]),
    (&["командная строка", "командную строку", "консоль cmd", "cmd"], &["cmd", "командная строка"]),
    (&["терминал", "windows terminal"], &["wt", "терминал", "windows terminal"]),
    (&["павершелл", "пауэршелл", "powershell"], &["powershell", "windows powershell"]),
    (&["паинт", "пейнт", "paint"], &["mspaint", "paint"]),
    (&["хром", "гугл хром", "chrome"], &["chrome", "google chrome"]),
    (&["эдж", "edge", "майкрософт эдж"], &["msedge", "microsoft edge"]),
    (&["фаерфокс", "файрфокс", "firefox", "мозилла"], &["firefox", "mozilla firefox"]),
    (&["яндекс браузер", "яндекс"], &["browser", "yandex", "яндекс браузер"]),
    (&["диспетчер задач", "task manager"], &["taskmgr", "диспетчер задач"]),
    (&["панель управления", "control panel"], &["control", "панель управления"]),
    (&["редактор реестра", "реестр", "regedit"], &["regedit", "редактор реестра"]),
    (&["вс код", "вскод", "visual studio code", "vs code", "код"], &["code", "visual studio code"]),
    (&["ворд", "word"], &["winword", "word"]),
    (&["эксель", "excel"], &["excel"]),
    (&["пауэрпоинт", "powerpoint"], &["powerpnt", "powerpoint"]),
    (&["аутлук", "почта", "outlook"], &["outlook", "почта"]),
    (&["телеграм", "телеграмм", "telegram"], &["telegram"]),
    (&["дискорд", "discord"], &["discord"]),
    (&["спотифай", "spotify"], &["spotify"]),
    (&["стим", "steam"], &["steam"]),
    (&["обс", "obs"], &["obs64", "obs studio"]),
    (&["ножницы", "snipping tool", "скриншот"], &["snippingtool", "ножницы", "snipping tool"]),
    (&["экранная клавиатура"], &["osk", "экранная клавиатура"]),
    (&["службы"], &["services.msc", "службы"]),
    (&["диспетчер устройств"], &["devmgmt.msc", "диспетчер устройств"]),
    (&["управление дисками"], &["diskmgmt.msc", "управление дисками"]),
    (&["просмотр событий", "журнал событий"], &["eventvwr.msc", "просмотр событий"]),
    (&["планировщик заданий", "планировщик"], &["taskschd.msc", "планировщик заданий"]),
    (&["монитор ресурсов"], &["resmon", "монитор ресурсов"]),
    (&["сведения о системе", "msinfo"], &["msinfo32", "сведения о системе"]),
    (&["магазин", "microsoft store", "store"], &["microsoft store", "магазин"]),
    (&["фотографии", "фото", "photos"], &["фотографии", "photos"]),
    (&["камеру", "камера приложение"], &["камера", "camera"]),
];

/// Известные папки и синонимы.
pub const FOLDERS: &[(KnownFolder, &[&str])] = &[
    (KnownFolder::Downloads, &["загрузки", "downloads", "скачанное", "скачанные файлы", "папка загрузок", "закачки"]),
    (KnownFolder::Documents, &["документы", "documents", "мои документы"]),
    (KnownFolder::Desktop, &["рабочий стол", "desktop"]),
    (KnownFolder::Pictures, &["изображения", "pictures", "картинки", "фотографии", "мои рисунки"]),
    (KnownFolder::Music, &["музыка", "music", "моя музыка"]),
    (KnownFolder::Videos, &["видео", "videos", "мои видео", "фильмы"]),
    (KnownFolder::Profile, &["домашняя папка", "профиль пользователя", "папка пользователя", "home"]),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enough_settings_pages() {
        assert!(SETTINGS.len() >= 30, "{}", SETTINGS.len());
        for (uri, syn) in SETTINGS {
            assert!(uri.ends_with(':') || uri.starts_with("ms-settings:"));
            assert!(syn.len() >= 2);
        }
    }
}
