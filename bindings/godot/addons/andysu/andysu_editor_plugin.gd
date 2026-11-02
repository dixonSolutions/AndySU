@tool
extends EditorPlugin

var export_plugin: AndySUExportPlugin

func _enter_tree():
	export_plugin = AndySUExportPlugin.new()
	add_export_plugin(export_plugin)
	add_autoload_singleton("AndySU", "res://addons/andysu/andysu.gd")

func _exit_tree():
	remove_export_plugin(export_plugin)
	remove_autoload_singleton("AndySU")

class AndySUExportPlugin extends EditorExportPlugin:
	func _supports_platform(platform):
		return platform is EditorExportPlatformAndroid

	func _get_android_libraries(platform, debug):
		return PackedStringArray(["andysu/bin/andysu-godot.aar"])

	func _get_name():
		return "AndySU"
