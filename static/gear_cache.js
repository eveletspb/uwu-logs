const CONTROLLER = new AbortController();

class FetchCache {
  constructor(server) {
    this.server = server;
    this.id = Date.now();
    this.CACHES = {
      class_set: {},
      icon: {},
      item: {},
      enchant: {},
    };
  }
  async fetch_item_data(id) {
    return this._fetch_wrap("item", id);
  }
  async fetch_enchant_data(id) {
    return this._fetch_wrap("enchant", id);
  }
  async fetch_class_set(class_name) {
    class_name = class_name.toLowerCase().replace(" ", "");
    if (!this.CACHES.class_set[class_name]) {
      await this._fetch_class_set(class_name);
    }
    return this.CACHES.class_set[class_name];
  }
  async icon_exists(id) {
    const cache = this.CACHES.icon;
    if (!cache[id]) {
      cache[id] = this._fetch_icon_exists(id);
    }
    return cache[id];
  }

  async _fetch_json(type, id) {
    const url = this.asset_url(type, id + ".json");
    const response = await fetch(url, {signal: CONTROLLER.signal});
    if (response.ok) return response.json();
    const local = await fetch(`/static/${type}/${encodeURIComponent(id)}.json`);
    if (local.ok) return local.json();
  }
  asset_url(type, filename) {
    return `/gear-assets/${encodeURIComponent(this.server)}/${type}/${encodeURIComponent(filename)}`;
  }
  async _fetch_missing_check_exists(type, id) {
    const query = new URLSearchParams({server: this.server, type, id});
    const missing_response = await fetch(`/missing?${query}`, {method: "PUT"});
    return [200, 201, 409].includes(missing_response.status);
  }
  async _fetch_json_wrap(type, id) {
    const fetch_try_1 = await this._fetch_json(type, id);
    if (fetch_try_1) return fetch_try_1;

    const missing_created = await this._fetch_missing_check_exists(type, id);
    if (missing_created) {
      const fetch_try_2 = await this._fetch_json(type, id);
      if (fetch_try_2) return fetch_try_2;
    } 

    return undefined;
  }
  async _fetch_wrap(type, id) {
    const cache = this.CACHES[type];
    if (!cache[id]) {
      cache[id] = this._fetch_json_wrap(type, id);
    }
    return cache[id];
  }
  async _fetch_class_set(class_name) {
    const response = await fetch(`/static/sets/${class_name}.json`);
    this.CACHES.class_set[class_name] = response.json();
  }
  async _fetch_icon_exists(id) {
    const file = id + ".jpg";
    const local = `/static/icons/${encodeURIComponent(file)}`;
    if ((await fetch(local)).ok) return local;
    const remote = this.asset_url("icons", file);
    if ((await fetch(remote)).ok) return remote;
    if (await this._fetch_missing_check_exists("icon", id) && (await fetch(remote)).ok) return remote;
    return "/static/icons/undefined.jpg";
  }
}

const realms = new Map();
export function gearCache(server = document.getElementById("gear-wrap")?.dataset.server ?? "") {
  if (!realms.has(server)) realms.set(server, new FetchCache(server));
  return realms.get(server);
}
