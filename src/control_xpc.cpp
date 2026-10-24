// native XPC ownership and Blocks stay behind a byte-oriented C interface.
// no callback performs filesystem work or waits for a worker.
#include <xpc/xpc.h>
#include <dispatch/dispatch.h>
#include <unistd.h>

#include <array>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <cstdint>
#include <cstring>
#include <memory>
#include <mutex>
#include <vector>

namespace {
enum Status { ok, unavailable, timeout, wrong_peer, protocol, capacity, too_large, internal };
using Receive = void (*)(const uint8_t *, size_t, uint32_t, uint32_t, void *, void *);
using Destroy = void (*)(void *);
std::atomic<size_t> live_clients{0};
std::atomic<size_t> live_replies{0};
std::atomic<size_t> authenticated_clients{0};

struct Client {
  std::mutex mutex;
  std::condition_variable changed;
  xpc_connection_t connection = nullptr;
  bool done = false;
  bool sent = false;
  bool authenticated = false;
  int status = unavailable;
  uint32_t uid = 0;
  uint32_t pid = 0;
  size_t limit;
  std::chrono::steady_clock::time_point deadline;
  std::vector<uint8_t> request;
  std::vector<uint8_t> response;

  Client(size_t body_limit, std::chrono::steady_clock::time_point end)
      : limit(body_limit), deadline(end) { ++live_clients; }
  ~Client() {
    if (connection) { xpc_connection_cancel(connection); xpc_release(connection); }
    if (authenticated) --authenticated_clients;
    --live_clients;
  }
};

void finish(const std::shared_ptr<Client> &client, int status) {
  std::lock_guard<std::mutex> lock(client->mutex);
  if (!client->done) {
    client->done = true;
    client->status = status;
    client->changed.notify_one();
  }
}

void received_body(const std::shared_ptr<Client> &client, xpc_object_t message,
                   uint32_t expected_uid) {
  if (xpc_get_type(message) != XPC_TYPE_DICTIONARY) {
    finish(client, unavailable);
    return;
  }
  if (xpc_connection_get_euid(client->connection) != expected_uid) {
    finish(client, wrong_peer);
    return;
  }
  auto failure = xpc_dictionary_get_value(message, "error");
  if (failure && xpc_get_type(failure) == XPC_TYPE_INT64) {
    const auto code = xpc_int64_get_value(failure);
    finish(client, code >= unavailable && code <= internal ? static_cast<int>(code) : protocol);
    return;
  }
  auto data = xpc_dictionary_get_value(message, "body");
  if (xpc_dictionary_get_count(message) != 1 || !data || xpc_get_type(data) != XPC_TYPE_DATA) {
    finish(client, protocol);
    return;
  }
  const auto length = xpc_data_get_length(data);
  if (length > client->limit) {
    finish(client, too_large);
    return;
  }
  try {
    std::lock_guard<std::mutex> lock(client->mutex);
    if (!client->done) {
      client->response.resize(length);
      if (length) std::memcpy(client->response.data(), xpc_data_get_bytes_ptr(data), length);
      client->done = true;
      client->status = ok;
      client->changed.notify_one();
    }
  } catch (...) { finish(client, capacity); }
}

int exchange(const char *name, const uint8_t *request, size_t length, uint64_t timeout_ns,
             uint32_t expected_uid, uint8_t *output, size_t limit, size_t *output_length,
             uint32_t *peer_uid, uint32_t *peer_pid, bool *sent, int fault) {
  const auto deadline = std::chrono::steady_clock::now() + std::chrono::nanoseconds(timeout_ns);
  std::shared_ptr<Client> client;
  try {
    client = std::make_shared<Client>(limit, deadline);
    if (length > limit) return too_large;
    if (length) client->request.assign(request, request + length);
    auto queue = dispatch_get_global_queue(QOS_CLASS_USER_INITIATED, 0);
    client->connection = xpc_connection_create_mach_service(name, queue, 0);
    if (!client->connection) return unavailable;
    std::weak_ptr<Client> weak = client;
    xpc_connection_set_event_handler(client->connection, ^(xpc_object_t event) {
      if (xpc_get_type(event) == XPC_TYPE_ERROR) {
        if (auto live = weak.lock()) finish(live, unavailable);
      }
    });
    xpc_connection_activate(client->connection);

    // authenticate the peer before sending invocation data. Both phases share one deadline.
    auto hello = xpc_dictionary_create(nullptr, nullptr, 0);
    xpc_dictionary_set_uint64(hello, "protocol", 1);
    xpc_connection_send_message_with_reply(client->connection, hello, queue, ^(xpc_object_t reply) {
      try {
      if (xpc_get_type(reply) != XPC_TYPE_DICTIONARY) {
        finish(client, unavailable);
        return;
      }
      const auto uid = xpc_connection_get_euid(client->connection);
      if (uid != expected_uid) {
        finish(client, wrong_peer);
        return;
      }
      if (xpc_dictionary_get_count(reply) != 1 ||
          xpc_dictionary_get_uint64(reply, "protocol") != 1) {
        finish(client, protocol);
        return;
      }
      std::lock_guard<std::mutex> lock(client->mutex);
      if (client->done) return;
      if (std::chrono::steady_clock::now() >= client->deadline) {
        client->done = true;
        client->status = timeout;
        client->changed.notify_one();
        return;
      }
      client->uid = uid;
      client->pid = static_cast<uint32_t>(xpc_connection_get_pid(client->connection));
      client->authenticated = true;
      ++authenticated_clients;
      if (fault == 3) return;
      auto message = xpc_dictionary_create(nullptr, nullptr, 0);
      if (fault == 1) {
        xpc_dictionary_set_bool(message, "unexpected", true);
      } else if (fault == 2) {
        std::vector<uint8_t> oversized(client->limit + 1, 0);
        xpc_dictionary_set_data(message, "body", oversized.data(), oversized.size());
      } else {
        xpc_dictionary_set_data(message, "body", client->request.data(), client->request.size());
      }
      client->sent = true;
      xpc_connection_send_message_with_reply(client->connection, message, queue, ^(xpc_object_t result) {
        received_body(client, result, expected_uid);
      });
      xpc_release(message);
      } catch (...) { finish(client, capacity); }
    });
    xpc_release(hello);

    std::unique_lock<std::mutex> lock(client->mutex);
    if (!client->changed.wait_until(lock, deadline, [&] { return client->done; })) {
      client->done = true;
      client->status = timeout;
    }
    *sent = client->sent;
    *peer_uid = client->uid;
    *peer_pid = client->pid;
    *output_length = client->response.size();
    if (!client->response.empty())
      std::memcpy(output, client->response.data(), client->response.size());
    const auto status = client->status;
    lock.unlock();
    xpc_connection_cancel(client->connection);
    return status;
  } catch (...) {
    if (client && client->connection) xpc_connection_cancel(client->connection);
    *sent = true;
    return capacity;
  }
}

struct PeerSlot {
  xpc_connection_t connection = nullptr;
  unsigned phase = 0;
  std::chrono::steady_clock::time_point deadline;
};

struct Service {
  dispatch_queue_t queue = nullptr;
  dispatch_source_t timer = nullptr;
  std::atomic<bool> alive{true};
  std::atomic<size_t> pending{0};
  std::vector<PeerSlot> peers;
  uint32_t uid;
  size_t body_limit;
  size_t pending_limit;
  Receive receive;
  void *context;
  Destroy destroy;

  Service(uint32_t user, size_t body, size_t clients, size_t requests, Receive callback,
          void *opaque, Destroy destructor)
      : peers(clients), uid(user), body_limit(body), pending_limit(requests),
        receive(callback), context(opaque), destroy(destructor) {}
  ~Service() {
    if (context) destroy(context);
    if (timer) { dispatch_source_cancel(timer); dispatch_release(timer); }
    if (queue) dispatch_release(queue);
  }
};

struct Pending {
  std::shared_ptr<Service> service;
  xpc_connection_t peer;
  xpc_object_t response;
  bool deferred = false;

  Pending(std::shared_ptr<Service> state, xpc_connection_t connection, xpc_object_t message)
      : service(std::move(state)), peer(connection), response(message) {
    xpc_retain(peer);
    xpc_retain(response);
    ++live_replies;
  }
  ~Pending() {
    xpc_release(response);
    xpc_release(peer);
    if (deferred) --service->pending;
    --live_replies;
  }
};

struct Listener {
  std::shared_ptr<Service> service;
  xpc_connection_t connection;
};

void reply_error(xpc_connection_t peer, xpc_object_t request, int status) {
  auto reply = xpc_dictionary_create_reply(request);
  if (!reply) return;
  xpc_dictionary_set_int64(reply, "error", status);
  xpc_connection_send_message(peer, reply);
  xpc_release(reply);
}

void receive(const std::shared_ptr<Service> &service, xpc_connection_t peer, xpc_object_t event) {
  if (xpc_get_type(event) != XPC_TYPE_DICTIONARY) return;
  if (!service->alive) { reply_error(peer, event, unavailable); return; }
  if (xpc_dictionary_get_count(event) == 1 &&
      xpc_dictionary_get_uint64(event, "protocol") == 1) {
    auto reply = xpc_dictionary_create_reply(event);
    if (reply) {
      xpc_dictionary_set_uint64(reply, "protocol", 1);
      xpc_connection_send_message(peer, reply);
      xpc_release(reply);
    }
    return;
  }
  auto data = xpc_dictionary_get_value(event, "body");
  if (xpc_dictionary_get_count(event) != 1 || !data || xpc_get_type(data) != XPC_TYPE_DATA) {
    reply_error(peer, event, protocol);
    return;
  }
  const auto length = xpc_data_get_length(data);
  if (length > service->body_limit) { reply_error(peer, event, too_large); return; }
  // a one-way sender cannot acquire an asynchronous reply/admission slot.
  auto reply = xpc_dictionary_create_reply(event);
  if (!reply) return;
  Pending *pending;
  try { pending = new Pending(service, peer, reply); }
  catch (...) {
    xpc_dictionary_set_int64(reply, "error", capacity);
    xpc_connection_send_message(peer, reply);
    xpc_release(reply);
    return;
  }
  xpc_release(reply);
  service->receive(static_cast<const uint8_t *>(xpc_data_get_bytes_ptr(data)), length,
                   xpc_connection_get_euid(peer), static_cast<uint32_t>(xpc_connection_get_pid(peer)),
                   pending, service->context);
}
}  // namespace

extern "C" int reap_xpc_exchange(
    const char *name, const uint8_t *request, size_t length, uint64_t timeout_ns, uint32_t uid,
    uint8_t *output, size_t output_capacity, size_t *output_length, uint32_t *peer_uid,
    uint32_t *peer_pid, bool *sent) {
  return exchange(name, request, length, timeout_ns, uid, output, output_capacity,
                  output_length, peer_uid, peer_pid, sent, 0);
}

// native malformed-wire controls are referenced only by Rust conformance tests.
extern "C" int reap_xpc_fault_exchange(
    const char *name, int fault, uint64_t timeout_ns, uint32_t uid, uint8_t *output,
    size_t output_capacity, size_t *output_length, uint32_t *peer_uid, uint32_t *peer_pid, bool *sent) {
  return exchange(name, nullptr, 0, timeout_ns, uid, output, output_capacity,
                  output_length, peer_uid, peer_pid, sent, fault);
}

extern "C" void *reap_xpc_server_start(
    const char *name, uint32_t uid, size_t body_limit, size_t client_limit, size_t pending_limit,
    Receive callback, void *context, Destroy destroy) {
  std::shared_ptr<Service> service;
  xpc_connection_t connection = nullptr;
  Listener *listener = nullptr;
  try {
    service = std::make_shared<Service>(uid, body_limit, client_limit, pending_limit,
                                        callback, context, destroy);
    service->queue = dispatch_queue_create("dev.micro.reap.control.transport", DISPATCH_QUEUE_SERIAL);
    if (!service->queue) return nullptr;
    connection = xpc_connection_create_mach_service(name, service->queue,
                                                       XPC_CONNECTION_MACH_SERVICE_LISTENER);
    if (!connection) return nullptr;
    listener = new Listener{service, connection};
    service->timer = dispatch_source_create(DISPATCH_SOURCE_TYPE_TIMER, 0, 0, service->queue);
    if (!service->timer) throw std::bad_alloc();
    std::weak_ptr<Service> weak = service;
    dispatch_source_set_event_handler(service->timer, ^{
      if (auto live = weak.lock()) {
        const auto now = std::chrono::steady_clock::now();
        for (const auto &slot : live->peers)
          if (slot.connection && now >= slot.deadline) xpc_connection_cancel(slot.connection);
      }
    });
    dispatch_source_set_timer(service->timer, dispatch_time(DISPATCH_TIME_NOW, 10 * NSEC_PER_MSEC),
                              10 * NSEC_PER_MSEC, NSEC_PER_MSEC);
    dispatch_activate(service->timer);
    xpc_connection_set_event_handler(connection, ^(xpc_object_t event) {
      if (xpc_get_type(event) != XPC_TYPE_CONNECTION) return;
      auto peer = static_cast<xpc_connection_t>(event);
      if (!service->alive || xpc_connection_get_euid(peer) != service->uid) {
        xpc_connection_cancel(peer);
        return;
      }
      size_t slot = 0;
      while (slot < service->peers.size() && service->peers[slot].connection) ++slot;
      if (slot == service->peers.size()) { xpc_connection_cancel(peer); return; }
      service->peers[slot] = PeerSlot{peer, 0,
        std::chrono::steady_clock::now() + std::chrono::seconds(1)};
      xpc_connection_set_target_queue(peer, service->queue);
      xpc_connection_set_event_handler(peer, ^(xpc_object_t message) {
        if (message == XPC_ERROR_CONNECTION_INVALID) {
          service->peers[slot].connection = nullptr;
          return;
        }
        if (xpc_get_type(message) == XPC_TYPE_DICTIONARY) {
          const bool hello = xpc_dictionary_get_count(message) == 1 &&
                             xpc_dictionary_get_uint64(message, "protocol") == 1;
          auto &phase = service->peers[slot].phase;
          if (phase >= 2) { xpc_connection_cancel(peer); return; }
          if ((phase == 0) != hello) {
            phase = 2;
            reply_error(peer, message, protocol);
            return;
          }
          ++phase;
        }
        receive(service, peer, message);
      });
      xpc_connection_activate(peer);
    });
    xpc_connection_activate(connection);
    return listener;
  } catch (...) {
    if (connection) {
      // inactive XPC connections need activation before cancellation and release.
      xpc_connection_set_event_handler(connection, ^(xpc_object_t) {});
      xpc_connection_activate(connection);
      xpc_connection_cancel(connection);
      xpc_release(connection);
    }
    delete listener;
    if (!service) destroy(context);
    return nullptr;
  }
}

extern "C" void reap_xpc_server_stop(void *opaque) {
  std::unique_ptr<Listener> listener(static_cast<Listener *>(opaque));
  auto service = listener->service;
  service->alive = false;
  dispatch_source_cancel(service->timer);
  dispatch_async(service->queue, ^{
    for (const auto &slot : service->peers)
      if (slot.connection) xpc_connection_cancel(slot.connection);
    // release handler-owned replies after callbacks finish, without a Service ownership cycle.
    auto context = service->context;
    service->context = nullptr;
    if (context) service->destroy(context);
  });
  xpc_connection_cancel(listener->connection);
  xpc_release(listener->connection);
}

extern "C" void reap_xpc_reply(void *opaque, const uint8_t *body, size_t length, int status) {
  std::unique_ptr<Pending> pending(static_cast<Pending *>(opaque));
  if (length > pending->service->body_limit) status = too_large;
  if (status) xpc_dictionary_set_int64(pending->response, "error", status);
  else xpc_dictionary_set_data(pending->response, "body", body, length);
  xpc_connection_send_message(pending->peer, pending->response);
}

extern "C" bool reap_xpc_reply_defer(void *opaque) {
  auto pending = static_cast<Pending *>(opaque);
  if (pending->deferred || !pending->service->alive) return false;
  if (pending->service->pending.fetch_add(1) >= pending->service->pending_limit) {
    --pending->service->pending;
    return false;
  }
  pending->deferred = true;
  return true;
}

// these counters expose native ownership to conformance probes, not cleanup policy.
extern "C" void reap_xpc_live_handles(size_t *clients, size_t *replies, size_t *authenticated) {
  *clients = live_clients.load();
  *replies = live_replies.load();
  *authenticated = authenticated_clients.load();
}
