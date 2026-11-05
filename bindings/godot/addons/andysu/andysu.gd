## AndySU autoload: update status and control. On platforms without AndySU every call is a no-op.
extends Node

signal status_changed(status: Dictionary)

var _p = null

func _ready():
	if Engine.has_singleton("AndySU"):
		_p = Engine.get_singleton("AndySU")
		_p.connect("status_changed", func(json): status_changed.emit(JSON.parse_string(json)))
		_p.listen()

func is_present() -> bool:
	return _p != null and _p.is_present()

func version() -> String:
	return _p.version() if _p else ""

func status() -> Dictionary:
	return JSON.parse_string(_p.status()) if _p else {"state": "idle"}

func check_now() -> void:
	if _p: _p.check_now()

func apply_now() -> void:
	if _p: _p.apply_now()

func restart_now() -> void:
	if _p: _p.restart_now()

## Commit this payload once the game is known to work (otherwise the host does it after a while).
func mark_healthy() -> void:
	if _p: _p.mark_healthy()

## "live", "restart", "next_start", or "" for the configured policy.
func set_apply_policy(policy: String) -> void:
	if _p: _p.set_apply_policy(policy)

## "auto", "download_only", "check_only", "off", or "" for the mode configured in the host. Needs
## Host API level 3; does nothing on an older host.
func set_update_mode(mode: String) -> void:
	if _p: _p.set_update_mode(mode)

## The effective update mode, or "" without a host or on a host older than API level 3.
func update_mode() -> String:
	return _p.update_mode() if _p else ""
