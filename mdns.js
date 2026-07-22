'use strict';
// Объявляем ПК в локальной сети по mDNS/Bonjour под постоянным именем
// `<name>.local`. Телефон один раз добавляет http://<name>.local:PORT/ в
// закладку — и адрес больше не зависит от смены IP по DHCP.
//
// Почему свой ответчик, а не bonjour-service: тот объявляет A-записи по ВСЕМ
// интерфейсам сразу (включая Hyper-V/WSL/VPN/VirtualBox), и телефон может
// выбрать нерабочий адрес. Здесь отвечаем только реальным LAN-адресом (тем же,
// что показывает сервер) и, привязавшись к его интерфейсу, шлём ответ именно
// через него — на машине с виртуальными адаптерами это критично.

let makeMdns;
try {
  makeMdns = require('multicast-dns');
} catch (e) {
  console.warn('[mdns] multicast-dns недоступен — объявление в сети выключено:', e.message);
}

const nameEq = (a, b) => String(a).toLowerCase() === String(b).toLowerCase();

// advertise({ name, getAddresses }):
//   name         — базовое имя, отвечаем на `<name>.local`
//   getAddresses — () => string[] текущих LAN IPv4 (уже отфильтрованных)
// Возвращает { hostname, available, stop() }.
function advertise({ name = 'sound-stream', getAddresses }) {
  const hostname = `${name}.local`;
  if (!makeMdns) return { hostname, available: false, stop() {} };

  let mdns = null;
  let boundIP = null;

  // Адрес вычисляется на момент ответа — переезд на новый IP подхватывается сам.
  // flush:true просит устройства заменить закэшированную запись, а не копить.
  const answers = () =>
    (getAddresses() || []).map(ip => ({ name: hostname, type: 'A', ttl: 120, flush: true, data: ip }));

  const announce = () => {
    const a = answers();
    if (mdns && a.length) mdns.respond({ answers: a });
  };

  const onQuery = (query) => {
    for (const q of (query.questions || [])) {
      if ((q.type === 'A' || q.type === 'ANY') && nameEq(q.name, hostname)) {
        const a = answers();
        if (a.length) mdns.respond({ answers: a });
        return; // на пакет отвечаем один раз
      }
    }
  };

  const bind = (ip) => {
    boundIP = ip || null;
    // Есть LAN-адрес → привязываемся к его интерфейсу: bind на 0.0.0.0 (чтобы
    // на Windows принимался мультикаст), а членство в группе и исходящий
    // интерфейс — на нужном LAN-адаптере. Нет адреса → дефолт по всем.
    mdns = makeMdns(ip ? { interface: ip, bind: '0.0.0.0' } : {});
    mdns.on('query', onQuery);
    mdns.on('error', e => console.warn('[mdns] ошибка сокета:', e.message));
    mdns.on('warning', () => {});
    announce(); // проактивный анонс — заранее прогреть кэши устройств
  };

  bind((getAddresses() || [])[0]);

  // Следим за сменой IP (DHCP): при смене основного LAN-адреса пересобираем
  // сокет под новый интерфейс; иначе просто периодически переанонсируем.
  const timer = setInterval(() => {
    const ip = (getAddresses() || [])[0] || null;
    if (ip !== boundIP) {
      console.log(`[mdns] LAN-адрес сменился (${boundIP || '—'} → ${ip || '—'}) — перепривязка`);
      try { mdns && mdns.destroy(); } catch (_) {}
      bind(ip);
    } else {
      announce();
    }
  }, 30000);
  if (timer.unref) timer.unref();

  return {
    hostname,
    available: true,
    stop() {
      clearInterval(timer);
      try { mdns && mdns.destroy(); } catch (_) {}
    }
  };
}

module.exports = { advertise };
