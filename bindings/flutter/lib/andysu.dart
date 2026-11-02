/// AndySU for Flutter: update status and control. Without an AndySU host (iOS, desktop, a
/// debug build) every call returns neutral values.
library andysu;

import 'dart:async';
import 'dart:convert';

import 'package:flutter/services.dart';

enum ApplyPolicy { live, restart, nextStart }

class AndySU {
  static const MethodChannel _ch = MethodChannel('andysu');
  static const EventChannel _ev = EventChannel('andysu/status');

  static Future<Map<String, dynamic>> _json(String method) async {
    try {
      final s = await _ch.invokeMethod<String>(method);
      return s == null ? <String, dynamic>{} : jsonDecode(s) as Map<String, dynamic>;
    } on MissingPluginException {
      return <String, dynamic>{};
    }
  }

  /// {present, apiLevel, version, isFactory, slotDir}
  static Future<Map<String, dynamic>> info() async {
    final m = await _json('info');
    return m.isEmpty ? {'present': false} : m;
  }

  static Future<Map<String, dynamic>> status() async {
    final m = await _json('status');
    return m.isEmpty ? {'state': 'idle'} : m;
  }

  static Future<void> _call(String m, [Object? args]) async {
    try {
      await _ch.invokeMethod<void>(m, args);
    } on MissingPluginException {
      // not running under AndySU
    }
  }

  static Future<void> checkNow() => _call('checkNow');
  static Future<void> applyNow() => _call('applyNow');
  static Future<void> restartNow() => _call('restartNow');
  static Future<void> markHealthy() => _call('markHealthy');

  /// How staged updates apply on this install; null returns to the configured policy.
  static Future<void> setApplyPolicy(ApplyPolicy? p) => _call('setApplyPolicy', switch (p) {
        ApplyPolicy.live => 'live',
        ApplyPolicy.restart => 'restart',
        ApplyPolicy.nextStart => 'next_start',
        null => null,
      });

  static Stream<Map<String, dynamic>> get statusChanges =>
      _ev.receiveBroadcastStream().map((s) => jsonDecode(s as String) as Map<String, dynamic>);
}
