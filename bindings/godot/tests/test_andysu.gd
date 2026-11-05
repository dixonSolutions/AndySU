## Runs the AndySU autoload script on a real engine. Without the Android plugin every call must be
## a no-op; with a stand-in registered under its name, calls and the status signal must get through.
extends SceneTree

class FakePlugin extends Object:
	signal status_changed(json: String)
	var calls: Array[String] = []

	func is_present() -> bool: return true
	func version() -> String: return "9.9"
	func status() -> String: return '{"state":"staged","release":"9.9"}'
	func listen() -> void: calls.append("listen")
	func check_now() -> void: calls.append("check_now")
	func apply_now() -> void: calls.append("apply_now")
	func restart_now() -> void: calls.append("restart_now")
	func mark_healthy() -> void: calls.append("mark_healthy")
	func set_apply_policy(policy: String) -> void: calls.append("policy:" + policy)
	func set_update_mode(mode: String) -> void: calls.append("mode:" + mode)
	func update_mode() -> String: return "download_only"

var failures := 0

func expect(what: String, got, want) -> void:
	if got != want:
		failures += 1
		printerr("FAIL %s: got %s, want %s" % [what, got, want])

func load_addon() -> Node:
	var node: Node = load("res://addons/andysu/andysu.gd").new()
	root.add_child(node)
	return node

func _initialize() -> void:
	var a := load_addon()
	expect("present without the plugin", a.is_present(), false)
	expect("version without the plugin", a.version(), "")
	expect("status without the plugin", a.status(), {"state": "idle"})
	expect("mode without the plugin", a.update_mode(), "")
	a.check_now()
	a.apply_now()
	a.restart_now()
	a.mark_healthy()
	a.set_apply_policy("live")
	a.set_update_mode("off")
	a.queue_free()

	var fake := FakePlugin.new()
	Engine.register_singleton("AndySU", fake)
	var b := load_addon()
	expect("present", b.is_present(), true)
	expect("version", b.version(), "9.9")
	expect("status", b.status(), {"state": "staged", "release": "9.9"})
	expect("mode", b.update_mode(), "download_only")
	b.check_now()
	b.apply_now()
	b.restart_now()
	b.mark_healthy()
	b.set_apply_policy("next_start")
	b.set_update_mode("check_only")
	expect("calls", fake.calls, ["listen", "check_now", "apply_now", "restart_now", "mark_healthy",
		"policy:next_start", "mode:check_only"] as Array[String])

	var seen := []
	b.status_changed.connect(func(s): seen.append(s))
	fake.status_changed.emit('{"state":"downloading"}')
	expect("status signal", seen, [{"state": "downloading"}])

	Engine.unregister_singleton("AndySU")
	b.queue_free()
	if failures == 0:
		print("andysu godot binding: ok")
	quit(1 if failures > 0 else 0)
