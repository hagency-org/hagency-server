#!/usr/bin/env python3
"""Validate reviewed OpenAPI contracts against current source; standard library only.

This is a static contract/drift guard, not a Rust compiler or full JSON Schema
validator. Router source hashes require deliberate re-review on routing changes;
route anchors and literal route inventory are checked separately. DTO property,
nullability, primitive/reference type and deny_unknown_fields checks are extracted
from Rust declarations. Supported examples are recursively schema-checked.
"""
import copy
import hashlib
import json
from pathlib import Path
import re
import sys
import unittest

ROOT = Path(__file__).resolve().parents[1]
FILES = [ROOT / 'crates/agent-service/openapi' / name for name in
         ['hagency-v1.openapi.json', 'appservice-v1.openapi.json']]
METHODS = {'get', 'put', 'post', 'delete', 'patch', 'options', 'head', 'trace'}


def require(condition, message):
    if not condition:
        raise ValueError(message)


def camel(name):
    return re.sub('_([a-z])', lambda m: m[1].upper(), name)


def resolve(doc, reference):
    require(reference.startswith('#/'), 'external reference is unsupported: ' + reference)
    value = doc
    for segment in reference[2:].split('/'):
        value = value[segment.replace('~1', '/').replace('~0', '~')]
    return value


def check_example(doc, schema, value):
    if '$ref' in schema:
        return check_example(doc, resolve(doc, schema['$ref']), value)
    if 'anyOf' in schema or 'oneOf' in schema:
        alternatives = schema.get('anyOf', schema.get('oneOf'))
        successes = 0
        for alternative in alternatives:
            try:
                check_example(doc, alternative, value)
                successes += 1
            except (ValueError, KeyError, TypeError):
                pass
        require(successes == 1 if 'oneOf' in schema else successes >= 1, 'example union mismatch')
        return
    if 'const' in schema:
        require(type(value) is type(schema['const']) and value == schema['const'], 'example const mismatch')
    if 'enum' in schema:
        require(value in schema['enum'], 'example enum mismatch')
    types = schema.get('type', [])
    types = [types] if isinstance(types, str) else types
    actual = ('null' if value is None else 'boolean' if isinstance(value, bool) else
              'integer' if isinstance(value, int) else 'number' if isinstance(value, float) else
              'string' if isinstance(value, str) else 'array' if isinstance(value, list) else
              'object' if isinstance(value, dict) else 'unknown')
    require(not types or actual in types or actual == 'integer' and 'number' in types, 'example type mismatch')
    if actual == 'object':
        require(all(k in value for k in schema.get('required', [])), 'example missing required field')
        properties = schema.get('properties', {})
        require(schema.get('additionalProperties') is not False or set(value) <= set(properties), 'example unknown field')
        for key, item in value.items():
            if key in properties:
                check_example(doc, properties[key], item)
    if actual == 'array':
        require('maxItems' not in schema or len(value) <= schema['maxItems'], 'example array too long')
        require(not schema.get('uniqueItems') or len({json.dumps(v, sort_keys=True) for v in value}) == len(value), 'example nonunique items')
        for item in value:
            check_example(doc, schema.get('items', {}), item)
    if actual in ['integer', 'number']:
        require('minimum' not in schema or value >= schema['minimum'], 'example below minimum')
        require('maximum' not in schema or value <= schema['maximum'], 'example above maximum')
    if actual == 'string':
        require('maxLength' not in schema or len(value) <= schema['maxLength'], 'example string too long')
        require('pattern' not in schema or re.search(schema['pattern'], value) is not None, 'example pattern mismatch')


def check_rust_schema(schema, metadata):
    source = (ROOT / metadata['file']).read_text()
    name = metadata['name']
    matches = list(re.finditer(r'(?:pub )?struct ' + re.escape(name) + r'\s*\{(.*?)\n\s*\}', source, re.S))
    if metadata.get('enum'):
        require('enum ' + name in source, 'missing enum ' + name)
        require('tag = "mode", rename_all = "snake_case", deny_unknown_fields' in source, 'Room policy tag/strictness drift')
        require(set(v['properties']['mode']['const'] for v in schema['oneOf']) == {'inherit_project', 'allow_list', 'disabled'}, 'Room policy variants drift')
        block = re.search(r'pub enum RoomCreationPolicy\s*\{(.*?)\n\}', source, re.S)[1]
        expected = {'InheritProject': {'deny'}, 'AllowList': {'allow', 'deny'}}
        for variant, fields in expected.items():
            body = re.search(variant + r'\s*\{([^}]+)\}', block)[1]
            parsed = dict(re.findall(r'(\w+): ([^,\n]+)', body))
            require(set(parsed) == fields and all(t == 'BTreeSet<String>' for t in parsed.values()), 'Room policy variant field/type drift')
            mode = {'InheritProject': 'inherit_project', 'AllowList': 'allow_list'}[variant]
            documented = next(s for s in schema['oneOf'] if s['properties']['mode']['const'] == mode)
            require(set(documented['properties']) == fields | {'mode'} and documented.get('additionalProperties') is False, 'Room policy variant schema drift')
        return
    require(len(matches) > metadata.get('occurrence', 0), 'missing Rust DTO ' + name)
    match = matches[metadata.get('occurrence', 0)]
    fields = []
    for item in re.finditer(r'((?:\s*#\[[^\n]+\]\s*)*)(?:pub )?(\w+): ([^,\n]+),', match[1]):
        attrs, field, typ = item.groups()
        if 'skip_serializing' not in attrs:
            fields.append((field, typ.strip()))
    require([f for f, _ in fields] == metadata['serializedFields'], 'Rust field drift ' + name)
    require(set(schema['properties']) == {camel(f) for f, _ in fields}, 'spec property drift ' + name)
    require(set(schema['required']) == set(schema['properties']), 'all Rust DTO fields are serialized/required: ' + name)
    if metadata.get('strictInput'):
        preceding = source[max(0, match.start() - 300):match.start()]
        require('deny_unknown_fields' in preceding.rsplit('#[derive', 1)[-1], 'Rust strictness drift ' + name)
        require(schema.get('additionalProperties') is False, 'spec strictness missing ' + name)
    for field, typ in fields:
        prop = schema['properties'][camel(field)]
        optional = typ.startswith('Option<')
        require(('anyOf' in prop) == optional, 'Rust nullable drift ' + name + '.' + field)
        if optional:
            require({'type': 'null'} in prop['anyOf'], 'missing explicit null type')
            typ = typ[7:-1]
            prop = next(s for s in prop['anyOf'] if s != {'type': 'null'})
        typ = typ.split('::')[-1]
        if typ.startswith('Vec<') and typ.endswith('>'):
            require(prop.get('type') == 'array' and prop.get('items', {}).get('$ref') == '#/components/schemas/' + typ[4:-1], 'Rust array type drift ' + name + '.' + field)
            continue
        expected = {'String': 'string', 'i64': 'integer', 'usize': 'integer', 'u64': 'integer', 'bool': 'boolean', 'BTreeSet<String>': 'array'}.get(typ)
        require(prop.get('type') == expected if expected else prop.get('$ref') == '#/components/schemas/' + typ, 'Rust type drift ' + name + '.' + field)


def validate(doc):
    require(doc['openapi'] == '3.1.0' and doc['info']['version'] == '1.0.0', 'version drift')
    def walk(value):
        if isinstance(value, dict):
            if '$ref' in value:
                resolve(doc, value['$ref'])
            for key, item in value.items():
                if key == 'examples' and isinstance(item, list):
                    for example in item:
                        check_example(doc, value, example)
                else:
                    walk(item)
        elif isinstance(value, list):
            for item in value:
                walk(item)
    walk(doc)
    for path, expected in doc['x-source-sha256'].items():
        require(hashlib.sha256((ROOT / path).read_bytes()).hexdigest() == expected, 'router source drift, re-review routes: ' + path)
    names = set()
    operations = set()
    for path, item in doc['paths'].items():
        require(path.startswith('/'), 'path must be absolute')
        for method, operation in item.items():
            require(method in METHODS, 'invalid method')
            require(operation['operationId'] not in names, 'duplicate operationId')
            names.add(operation['operationId']); operations.add((path, method))
            require(set(re.findall(r'{(\w+)}', path)) == {p['name'] for p in operation['parameters'] if p['in'] == 'path' and p['required']}, 'path parameter drift')
            source = (ROOT / operation['x-source']['file']).read_text()
            for anchor in operation['x-source']['anchors']:
                require(' '.join(anchor.split()) in ' '.join(source.split()), 'route/source anchor missing: ' + path + ' ' + anchor)
            anchors = ' '.join(operation['x-source']['anchors'])
            documented_guards = re.findall(r'Method::(GET|POST|PUT|DELETE|PATCH|HEAD|OPTIONS)', anchors)
            require(not documented_guards or method.upper() in documented_guards, 'method disagrees with source guard: ' + path)
            require('responses' in operation and operation['responses'], 'no responses')
            for security in operation['security']:
                require(set(security) <= set(doc['components']['securitySchemes']), 'security scheme missing')
            if path.startswith('/api/hagency/v1/execution/'):
                require(operation['security'] == [{'deviceBearer': []}] and method == 'post', 'execution bearer/method drift')
            elif (path == '/api/hagency/v1/agents' and method == 'post') or (path == '/api/hagency/v1/agents/{agentId}/execution-device' and method == 'put'):
                require(operation['security'] == [{'deviceBearer': []}], 'device management security drift')
            elif path.startswith('/api/hagency/v1/') and path not in ['/api/hagency/v1/discovery', '/api/hagency/v1/readiness', '/api/hagency/v1/sessions/pasion']:
                require(operation['security'] == [{'userBearer': []}], 'owner session security drift')
    for schema in doc['components']['schemas'].values():
        if 'x-rust-dto' in schema:
            check_rust_schema(schema, schema['x-rust-dto'])
    if 'userBearer' in doc['components']['securitySchemes']:
        api = (ROOT / 'crates/agent-service/src/api.rs').read_text()
        transport = (ROOT / 'crates/agent-service/src/api_transport.rs').read_text()
        literal = set((p, m.lower()) for p, m in re.findall(r'path\s*==\s*"(/api/hagency/v1/[^"]+)"\s*&&\s*method\s*==\s*salvo::http::Method::(\w+)', api))
        literal |= {(p, 'post') for p in re.findall(r'"(/api/hagency/v1/execution/[^"]+)" =>', transport)}
        require(literal <= operations, 'source literal route omitted: ' + str(literal - operations))
        require({p for p, _ in operations if p.startswith('/api/hagency/v1/execution/')} == {p for p, _ in literal if p.startswith('/api/hagency/v1/execution/')}, 'invented transport route')
        require(('/api/hagency/v1/projects', 'post') not in operations, 'unimplemented Space creation advertised')
        require(('/api/hagency/v1/projects/{projectId}/rooms', 'post') not in operations, 'unimplemented Room creation advertised')
        require(len(operations) == 52, 'reviewed owner route count drift')
    else:
        require(len(operations) == 4, 'reviewed AS route count drift')
    return len(operations)


class GuardTests(unittest.TestCase):
    def setUp(self):
        self.doc = json.loads(FILES[0].read_text())
    def rejection(self, mutate):
        doc = copy.deepcopy(self.doc); mutate(doc)
        with self.assertRaises((ValueError, KeyError, TypeError)):
            validate(doc)
    def test_missing_real_route(self):
        self.rejection(lambda d: d['paths'].pop('/api/hagency/v1/execution/events/authorize-tool'))
    def test_wrong_method(self):
        self.rejection(lambda d: d['paths']['/api/hagency/v1/execution/events/ack'].update(get=d['paths']['/api/hagency/v1/execution/events/ack'].pop('post')))
    def test_wrong_bearer(self):
        self.rejection(lambda d: d['paths']['/api/hagency/v1/execution/events/start']['post'].update(security=[{'userBearer': []}]))
    def test_create_requires_current_device(self):
        self.rejection(lambda d: d['paths']['/api/hagency/v1/agents']['post'].update(security=[{'userBearer': []}]))
    def test_assign_requires_current_device(self):
        self.rejection(lambda d: d['paths']['/api/hagency/v1/agents/{agentId}/execution-device']['put'].update(security=[{'userBearer': []}]))
    def test_private_or_missing_field(self):
        self.rejection(lambda d: d['components']['schemas']['ReplyIntent']['properties'].update(workerToken={'type': 'string'}))
    def test_rust_type_drift(self):
        self.rejection(lambda d: d['components']['schemas']['DeviceGrant']['properties']['generation'].update(type='string', examples=['bad']))
    def test_non_strict_request(self):
        self.rejection(lambda d: d['components']['schemas']['Start'].update(additionalProperties=True))
    def test_unresolved_ref(self):
        self.rejection(lambda d: d['components']['schemas']['Start']['properties'].update(lease={'$ref': '#/components/schemas/Unknown'}))
    def test_bad_example(self):
        self.rejection(lambda d: d['components']['schemas']['Finish'].update(examples=[{'lease': {}, 'dispatchId': 'x', 'executionId': 'e', 'outcome': 'success'}]))


if __name__ == '__main__':
    try:
        for path in FILES:
            count = validate(json.loads(path.read_text()))
            print(f'{path.relative_to(ROOT)}: {count} operations validated')
        if '--self-test' in sys.argv:
            result = unittest.TextTestRunner(stream=sys.stdout, verbosity=0).run(unittest.defaultTestLoader.loadTestsFromTestCase(GuardTests))
            sys.exit(0 if result.wasSuccessful() else 1)
    except (ValueError, KeyError, TypeError) as error:
        print('OpenAPI validation failed:', error, file=sys.stderr)
        sys.exit(1)
