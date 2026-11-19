(() => {
  const labels = {
    disabled: 'Автоимпорт на этом сервере выключен.',
    not_started: 'Автоимпорт ещё не запущен.',
    starting: 'Запуск автоимпорта…',
    waiting_for_log: 'Ожидание рейда. Автоимпорт включён.',
    recording: 'Идёт запись босса.',
    deferred_until_client_exit: 'WoW открыт. Разбор и очистка отложены до выхода из игры.',
    waiting_for_file_flush: 'Рейд завершён. Ожидание записи журнала на диск…',
    importing: 'Импорт рейда…',
    imported_rotation_pending_client_exit: 'Импорт завершён. Переименование журнала ожидает закрытия WoW.',
    rotation_waiting_for_file_unlock: 'Импорт завершён. Журнал пока занят клиентом.',
    rotating: 'Архивирование журнала…',
    ready: 'Готово. Журнал архивирован. Ожидание следующего рейда.',
    waiting_for_logging_status: 'Не удалось определить состояние записи. Исходный журнал сохранён.',
    error: 'Ошибка автоимпорта. Исходный журнал сохранён; попытка будет повторена.'
  };
  const banner = document.createElement('div');
  banner.id = 'local-status';
  banner.setAttribute('role', 'status');
  document.querySelector('nav').after(banner);
  async function refresh() {
    try {
      const response = await fetch('/local-status');
      const data = await response.json();
      banner.textContent = (labels[data.state] || 'Автоимпорт рейдов включён.')
        + (data.warning ? ' Журнал больше 3 ГБ: закройте WoW после рейда для архивирования.' : '');
    } catch (_) {
      banner.textContent = 'Нет связи с локальным автоимпортом.';
    }
  }
  refresh();
  setInterval(refresh, 5000);
})();
