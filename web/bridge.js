// Runs at document start in the main frame, before any WhatsApp code.
// - window.elecwhat: calls into the Rust side (see `handle_ipc` in src/app.rs)
// - window.Notification: replaced to show desktop notifications
// - favicon changes are reported to update the tray icon and unread badge
(() => {
  if (window.elecwhat) return;

  // replaced by the Rust side with a JSON object
  const state = __ELECWHAT_STATE__;

  const post = (method, ...args) => {
    try {
      window.ipc.postMessage(JSON.stringify({ method, args }));
    } catch (err) {
      console.error("elecwhat ipc", method, err);
    }
  };

  const api = {
    debug: !!state.debug,
    stateGet: async (name) => state[name],
    notifyEv: () => post("windowShow"),
    restore: () => post("windowShow"),
    open: (url) => post("open", String(url)),
    windowToggle: () => post("windowToggle"),
    windowShow: () => post("windowShow"),
    windowHide: () => post("windowHide"),
    reload: () => post("reload"),
    quit: () => post("quit"),
    zoomIn: () => post("zoomIn"),
    zoomOut: () => post("zoomOut"),
    zoomReset: () => post("zoomReset"),
    fullscreenToggle: () => post("fullscreenToggle"),
    devTools: () => post("devTools"),
  };
  Object.defineProperty(window, "elecwhat", { value: Object.freeze(api) });

  // --- Notifications ---
  const notifications = new Map();
  const fire = new WeakMap();
  let nextId = 1;

  class ElecwhatNotification extends EventTarget {
    static get permission() {
      return "granted";
    }
    static requestPermission(callback) {
      callback?.("granted");
      return Promise.resolve("granted");
    }
    static get maxActions() {
      return 0;
    }

    constructor(title, options = {}) {
      super();
      const id = nextId++;
      Object.defineProperties(this, {
        title: { value: String(title ?? ""), enumerable: true },
        body: { value: String(options.body ?? ""), enumerable: true },
        tag: { value: String(options.tag ?? ""), enumerable: true },
        icon: { value: String(options.icon ?? ""), enumerable: true },
        badge: { value: String(options.badge ?? ""), enumerable: true },
        lang: { value: String(options.lang ?? ""), enumerable: true },
        dir: { value: String(options.dir ?? "auto"), enumerable: true },
        silent: { value: !!options.silent, enumerable: true },
        renotify: { value: !!options.renotify, enumerable: true },
        requireInteraction: { value: !!options.requireInteraction, enumerable: true },
        data: { value: options.data ?? null, enumerable: true },
      });
      this.onclick = this.onclose = this.onshow = this.onerror = null;

      fire.set(this, (type) => {
        const ev = new Event(type, { cancelable: true });
        const handler = this[`on${type}`];
        if (typeof handler === "function") {
          try {
            handler.call(this, ev);
          } catch (err) {
            console.error(err);
          }
        }
        this.dispatchEvent(ev);
      });
      notifications.set(id, this);
      this._elecwhatId = id;

      post("notify", {
        id,
        title: `${state.notifPrefix ?? ""}${this.title}`,
        body: this.body,
        tag: this.tag || null,
        silent: this.silent,
      });
      setTimeout(() => fire.get(this)?.("show"), 0);
    }

    close() {
      const id = this._elecwhatId;
      if (notifications.delete(id)) {
        post("notifyClose", id);
        fire.get(this)?.("close");
      }
    }
  }

  // called by the Rust side: "click" or "close"
  Object.defineProperty(window, "__elecwhatNotificationEvent", {
    value: (id, type) => {
      const n = notifications.get(id);
      if (!n) return;
      if (type === "close") notifications.delete(id);
      console.debug("notification", type, id);
      fire.get(n)?.(type);
    },
  });

  window.Notification = ElecwhatNotification;

  // --- Favicon (the unread count) ---
  let lastIcon = null;
  const checkIcon = () => {
    const links = document.querySelectorAll('link[rel~="icon"]');
    const href = links.length ? links[links.length - 1].href : null;
    if (href && href !== lastIcon) {
      lastIcon = href;
      post("favicon", href);
    }
  };
  const watchHead = (head) => {
    new MutationObserver(checkIcon).observe(head, {
      subtree: true,
      childList: true,
      attributes: true,
      attributeFilter: ["href", "rel"],
    });
    checkIcon();
  };
  if (document.head) {
    watchHead(document.head);
  } else {
    const waitHead = new MutationObserver(() => {
      if (document.head) {
        waitHead.disconnect();
        watchHead(document.head);
      }
    });
    waitHead.observe(document.documentElement ?? document, { childList: true, subtree: true });
  }
})();
