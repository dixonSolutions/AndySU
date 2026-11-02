import 'package:andysu/andysu.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const ch = MethodChannel('andysu');
  final calls = <String>[];

  setUp(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger.setMockMethodCallHandler(ch, (c) async {
      calls.add('${c.method}:${c.arguments}');
      switch (c.method) {
        case 'info':
          return '{"present":true,"apiLevel":1,"version":"2.0","isFactory":false,"slotDir":"/s"}';
        case 'status':
          return '{"state":"staged","release":"2.1"}';
      }
      return null;
    });
  });

  test('bridges to the host', () async {
    expect((await AndySU.info())['version'], '2.0');
    expect((await AndySU.status())['release'], '2.1');
    await AndySU.setApplyPolicy(ApplyPolicy.nextStart);
    await AndySU.checkNow();
    expect(calls, contains('setApplyPolicy:next_start'));
    expect(calls, contains('checkNow:null'));
  });
}
