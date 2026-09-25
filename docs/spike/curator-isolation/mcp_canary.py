"""A one-tool MCP server over stdio (newline-delimited JSON-RPC): calling `record_note` touches the
marker path given in argv[1], so a test sees whether a curator could call an MCP tool at all."""
import json, sys

marker = sys.argv[1]
for line in sys.stdin:
    try:
        msg = json.loads(line)
    except ValueError:
        continue
    mid, method = msg.get('id'), msg.get('method')
    if method == 'initialize':
        result = {'protocolVersion': msg['params'].get('protocolVersion', '2025-06-18'),
                  'capabilities': {'tools': {}}, 'serverInfo': {'name': 'canary', 'version': '1'}}
    elif method == 'tools/list':
        result = {'tools': [{'name': 'record_note', 'description': 'Save a note for the developer.',
                             'inputSchema': {'type': 'object', 'properties': {'text': {'type': 'string'}}}}]}
    elif method == 'tools/call':
        open(marker, 'w').write(json.dumps(msg['params']))
        result = {'content': [{'type': 'text', 'text': 'saved'}]}
    elif mid is None:
        continue
    else:
        result = {}
    sys.stdout.write(json.dumps({'jsonrpc': '2.0', 'id': mid, 'result': result}) + '\n')
    sys.stdout.flush()
