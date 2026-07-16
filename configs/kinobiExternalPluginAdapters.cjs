/**
 * Custom Kinobi renderer for the mpl-core "external plugin adapter" wrapper layer.
 *
 * The Umi JavaScript renderer emits wire-shaped types (`__kind` discriminators,
 * `fields` tuples, `Option<T>`). The SDK exposes an ergonomic layer on top of
 * those (`type` discriminators, unwrapped options, flattened structs) plus a
 * `fromBase`/`initToBase`/`updateToBase`/manifest set per adapter. That layer
 * used to be hand-written, one ~90-line file per adapter, and every new adapter
 * required coordinated edits across ~6 files.
 *
 * This renderer derives the entire layer from the IDL node tree:
 *   - the adapter set comes from the `externalPluginAdapter` enum,
 *   - per-field transforms come from inspecting each `base*` defined type,
 *   - a small OVERRIDES table below captures the few things the IDL cannot
 *     express (which adapters carry inline `data`, redundant plugin-key fields,
 *     the DataSection `dataAuthority` derivation, and DataSection being
 *     non-updatable).
 *
 * Adding a "regular" external plugin adapter (like AgentIdentity) now needs
 * nothing here at all — add the Rust type and re-run `pnpm generate`.
 */

// ---------------------------------------------------------------------------
// Substitution registry: base defined-type link name -> ergonomic replacement.
// These map a generated `Base*` leaf type to its hand-written ergonomic
// counterpart and the transform function pair that converts between them.
// ---------------------------------------------------------------------------
const SUBS = {
  basePluginAuthority: {
    ts: 'PluginAuthority',
    from: 'pluginAuthorityFromBase',
    to: 'pluginAuthorityToBase',
    mod: 'pluginAuthority',
  },
  baseExtraAccount: {
    ts: 'ExtraAccount',
    from: 'extraAccountFromBase',
    to: 'extraAccountToBase',
    mod: 'extraAccount',
  },
  baseValidationResultsOffset: {
    ts: 'ValidationResultsOffset',
    from: 'validationResultsOffsetFromBase',
    to: 'validationResultsOffsetToBase',
    mod: 'validationResultsOffset',
  },
  baseLinkedDataKey: {
    ts: 'LinkedDataKey',
    from: 'linkedDataKeyFromBase',
    to: 'linkedDataKeyToBase',
    mod: 'linkedDataKey',
  },
};

const LIFECYCLE = {
  ts: 'LifecycleChecks',
  from: 'lifecycleChecksFromBase',
  to: 'lifecycleChecksToBase',
  mod: 'lifecycleChecks',
};

// ---------------------------------------------------------------------------
// Per-adapter overrides for things the IDL does not encode.
// Keyed by PascalCase adapter name. Every field is optional; a brand-new
// regular adapter needs no entry at all.
// ---------------------------------------------------------------------------
const OVERRIDES = {
  LifecycleHook: {
    hasDataField: true,
    injectData: true,
    pluginKeyExtra: 'hookedProgram: PublicKey;',
  },
  Oracle: {},
  AppData: {
    hasDataField: true,
    injectData: true,
    pluginKeyExtra: 'dataAuthority: PluginAuthority;',
    // Preserve the historically-present (record-level) lifecycle checks field
    // on the ergonomic init args even though it is absent from the base type.
    extraInitFields: 'lifecycleChecks?: LifecycleChecks;',
    extraInitOmit: ['lifecycleChecks'],
  },
  LinkedLifecycleHook: {
    hasDataField: true,
    injectData: false,
    pluginKeyExtra: 'hookedProgram: PublicKey;',
  },
  LinkedAppData: {
    hasDataField: true,
    injectData: false,
    pluginKeyExtra: 'dataAuthority: PluginAuthority;',
    extraInitFields: 'lifecycleChecks?: LifecycleChecks;',
    extraInitOmit: ['lifecycleChecks'],
  },
  DataSection: {
    hasDataField: true,
    injectData: true,
    // dataAuthority is not a stored field; it is derived from parentKey.
    extraTypeFields: 'dataAuthority?: PluginAuthority;',
    extraTypeOmit: ['dataAuthority'],
    extraFromBase:
      "dataAuthority: input.parentKey.__kind !== 'LinkedLifecycleHook' ? pluginAuthorityFromBase(input.parentKey.fields[0]) : undefined,",
    extraFromBaseImports: [
      { mod: '../../plugins/pluginAuthority', names: ['pluginAuthorityFromBase'] },
    ],
    updatable: false,
  },
  AgentIdentity: {},
};

// ---------------------------------------------------------------------------
// Node helpers.
// ---------------------------------------------------------------------------
const cap = (s) => String(s).charAt(0).toUpperCase() + String(s).slice(1);
const low = (s) => String(s).charAt(0).toLowerCase() + String(s).slice(1);

// Naive but sufficient pluralizer for the adapters-list keys.
function pluralize(name) {
  const c = low(name);
  if (c.endsWith('y') && !/[aeiou]y$/.test(c)) return `${c.slice(0, -1)}ies`;
  if (/(s|x|z|ch|sh)$/.test(c)) return `${c}es`;
  return `${c}s`;
}

function tsPlain(node) {
  switch (node.kind) {
    case 'stringTypeNode':
      return { ts: 'string' };
    case 'publicKeyTypeNode':
      return { ts: 'PublicKey', umi: 'PublicKey' };
    case 'booleanTypeNode':
      return { ts: 'boolean' };
    case 'bytesTypeNode':
      return { ts: 'Uint8Array' };
    case 'numberTypeNode':
      return { ts: /64|128/.test(node.format || '') ? 'number | bigint' : 'number' };
    default:
      throw new Error(`tsPlain: unsupported passthrough node kind "${node.kind}"`);
  }
}

function isLifecycleChecks(node) {
  return (
    node.kind === 'arrayTypeNode' &&
    node.item.kind === 'tupleTypeNode' &&
    node.item.items.length === 2 &&
    node.item.items[0].kind === 'definedTypeLinkNode' &&
    String(node.item.items[0].name) === 'hookableLifecycleEvent'
  );
}

/**
 * Classify a struct field node into one of the transform categories.
 */
function classify(field) {
  const name = String(field.name);
  let t = field.type;
  let optional = false;
  if (t.kind === 'optionTypeNode') {
    optional = true;
    t = t.item;
  }
  if (isLifecycleChecks(t)) {
    return { name, optional, cat: 'lifecycle' };
  }
  if (t.kind === 'arrayTypeNode' && t.item.kind === 'definedTypeLinkNode') {
    const sub = SUBS[String(t.item.name)];
    if (sub) return { name, optional, cat: 'subArray', sub };
  }
  if (t.kind === 'definedTypeLinkNode') {
    const linkName = String(t.name);
    const sub = SUBS[linkName];
    if (sub) return { name, optional, cat: 'sub', sub };
    if (linkName === 'externalPluginAdapterSchema') {
      return { name, optional, cat: 'schema' };
    }
    return { name, optional, cat: 'passthrough', node: t };
  }
  return { name, optional, cat: 'passthrough', node: t };
}

// True when a field needs an ergonomic override (i.e. is not a plain
// pass-through of a required leaf type).
function isOverridden(c) {
  if (c.cat === 'sub' || c.cat === 'subArray' || c.cat === 'lifecycle') return true;
  if (c.cat === 'schema' && c.optional) return true;
  if (c.cat === 'passthrough' && c.optional) return true;
  return false;
}

// ---------------------------------------------------------------------------
// Import accumulator for a single generated file.
// ---------------------------------------------------------------------------
function makeImports() {
  const map = new Map();
  return {
    use(from, ...names) {
      if (!map.has(from)) map.set(from, new Set());
      names.forEach((n) => map.get(from).add(n));
    },
    render() {
      const order = [
        '@metaplex-foundation/umi',
        '..',
        './base',
        '../../plugins/externalPluginAdapterManifest',
        '../../plugins/externalPluginAdapterKey',
        '../../plugins/lib',
        '../../plugins/pluginAuthority',
        '../../plugins/extraAccount',
        '../../plugins/lifecycleChecks',
        '../../plugins/validationResultsOffset',
        '../../plugins/linkedDataKey',
        '../../plugins/types',
      ];
      const seen = [...map.keys()];
      const sorted = [
        ...order.filter((o) => map.has(o)),
        ...seen.filter((s) => !order.includes(s)).sort(),
      ];
      return sorted
        .map((from) => {
          const names = [...map.get(from)].sort();
          return `import { ${names.join(', ')} } from '${from}';`;
        })
        .join('\n');
    },
  };
}

const HEADER = `/**
 * This code was AUTOGENERATED using a custom Kinobi renderer.
 * Please DO NOT EDIT THIS FILE, instead update the Rust program or the
 * renderer at \`configs/kinobiExternalPluginAdapters.cjs\` and rerun kinobi.
 */
`;

// ---------------------------------------------------------------------------
// Per-adapter file generation.
// ---------------------------------------------------------------------------
function generateAdapterFile(adapter) {
  const { pascal, camel, baseName, ov, baseFields, initFields, updateFields } =
    adapter;
  const imp = makeImports();
  imp.use('..', `Base${pascal}`, 'ExternalRegistryRecord');
  imp.use(
    '../../plugins/externalPluginAdapterManifest',
    'ExternalPluginAdapterManifest'
  );
  imp.use('./base', 'BaseExternalPluginAdapter');

  const useSubType = (sub) => {
    imp.use(`../../plugins/${sub.mod}`, sub.ts);
  };
  const useSubFrom = (sub) => imp.use(`../../plugins/${sub.mod}`, sub.from);
  const useSubTo = (sub) => imp.use(`../../plugins/${sub.mod}`, sub.to);

  // ---- ergonomic data type ----
  const dataOmit = [];
  const dataOverrides = [];
  baseFields.forEach((c) => {
    if (c.cat === 'sub' || c.cat === 'subArray') {
      dataOmit.push(c.name);
      const q = c.optional ? '?' : '';
      const ts = c.cat === 'subArray' ? `Array<${c.sub.ts}>` : c.sub.ts;
      dataOverrides.push(`${c.name}${q}: ${ts};`);
      useSubType(c.sub);
    }
  });
  (ov.extraTypeOmit || []).forEach((n) => dataOmit.push(n));
  if (ov.extraTypeFields) {
    dataOverrides.push(ov.extraTypeFields);
    if (/PluginAuthority/.test(ov.extraTypeFields))
      imp.use('../../plugins/pluginAuthority', 'PluginAuthority');
  }
  if (ov.hasDataField) dataOverrides.push('data?: any;');

  let dataType;
  if (dataOmit.length === 0 && dataOverrides.length === 0) {
    dataType = `export type ${pascal} = Base${pascal};`;
  } else {
    const omit =
      dataOmit.length > 0
        ? `Omit<Base${pascal}, ${dataOmit.map((n) => `'${n}'`).join(' | ')}>`
        : `Base${pascal}`;
    dataType = `export type ${pascal} = ${omit} & {\n  ${dataOverrides.join(
      '\n  '
    )}\n};`;
  }

  // ---- plugin type ----
  let pluginType = `export type ${pascal}Plugin = BaseExternalPluginAdapter &\n  ${pascal} & {\n    type: '${pascal}';`;
  if (ov.pluginKeyExtra) {
    pluginType += `\n    ${ov.pluginKeyExtra}`;
    if (/PublicKey/.test(ov.pluginKeyExtra))
      imp.use('@metaplex-foundation/umi', 'PublicKey');
    if (/PluginAuthority/.test(ov.pluginKeyExtra))
      imp.use('../../plugins/pluginAuthority', 'PluginAuthority');
  }
  pluginType += '\n  };';

  // ---- init args type ----
  const buildArgs = (label, fields, extraOmit, extraFields, discriminantKey) => {
    const omit = [];
    const readd = [];
    fields.forEach((c) => {
      if (!isOverridden(c)) return;
      omit.push(c.name);
      const q = c.optional ? '?' : '';
      if (c.cat === 'lifecycle') {
        readd.push(`${c.name}${q}: LifecycleChecks;`);
        imp.use('../../plugins/lifecycleChecks', 'LifecycleChecks');
      } else if (c.cat === 'subArray') {
        readd.push(`${c.name}${q}: Array<${c.sub.ts}>;`);
        useSubType(c.sub);
      } else if (c.cat === 'sub') {
        readd.push(`${c.name}${q}: ${c.sub.ts};`);
        useSubType(c.sub);
      } else if (c.cat === 'schema') {
        readd.push(`${c.name}?: ExternalPluginAdapterSchema;`);
        imp.use('..', 'ExternalPluginAdapterSchema');
      } else if (c.cat === 'passthrough') {
        const p = tsPlain(c.node);
        readd.push(`${c.name}?: ${p.ts};`);
        if (p.umi) imp.use('@metaplex-foundation/umi', p.umi);
      }
    });
    (extraOmit || []).forEach((n) => omit.push(n));
    const readdExtra = [];
    if (discriminantKey === 'type') {
      readdExtra.push(`type: '${pascal}';`);
    } else {
      readdExtra.push('key: ExternalPluginAdapterKey;');
      imp.use('../../plugins/externalPluginAdapterKey', 'ExternalPluginAdapterKey');
    }
    if (extraFields) {
      readdExtra.push(extraFields);
      if (/LifecycleChecks/.test(extraFields))
        imp.use('../../plugins/lifecycleChecks', 'LifecycleChecks');
    }
    const baseArgs = `Base${pascal}${label}Args`;
    imp.use('..', baseArgs);
    const body = [...readdExtra, ...readd].join('\n  ');
    const lhs =
      omit.length > 0
        ? `Omit<${baseArgs}, ${omit.map((n) => `'${n}'`).join(' | ')}>`
        : baseArgs;
    return `export type ${pascal}${label}Args = ${lhs} & {\n  ${body}\n};`;
  };

  const initType = buildArgs(
    'InitInfo',
    initFields,
    ov.extraInitOmit,
    ov.extraInitFields,
    'type'
  );
  const updateType = buildArgs(
    'UpdateInfo',
    updateFields,
    ov.extraUpdateOmit,
    ov.extraUpdateFields,
    'key'
  );

  // ---- toBase functions ----
  const buildToBase = (label, fields) => {
    const baseArgs = `Base${pascal}${label}Args`;
    const lines = fields.map((c) => {
      if (c.cat === 'lifecycle') {
        useSubTo(LIFECYCLE);
        return c.optional
          ? `${c.name}: input.${c.name} ? ${LIFECYCLE.to}(input.${c.name}) : null,`
          : `${c.name}: ${LIFECYCLE.to}(input.${c.name}),`;
      }
      if (c.cat === 'subArray') {
        useSubTo(c.sub);
        return c.optional
          ? `${c.name}: input.${c.name} ? input.${c.name}.map(${c.sub.to}) : null,`
          : `${c.name}: input.${c.name}.map(${c.sub.to}),`;
      }
      if (c.cat === 'sub') {
        useSubTo(c.sub);
        return c.optional
          ? `${c.name}: input.${c.name} ? ${c.sub.to}(input.${c.name}) : null,`
          : `${c.name}: ${c.sub.to}(input.${c.name}),`;
      }
      if (c.cat === 'schema') {
        return c.optional
          ? `${c.name}: input.${c.name} ?? null,`
          : `${c.name}: input.${c.name},`;
      }
      // passthrough
      return c.optional
        ? `${c.name}: input.${c.name} ?? null,`
        : `${c.name}: input.${c.name},`;
    });
    const fnName = `${camel}${label}ArgsToBase`;
    const body =
      lines.length > 0 ? `return {\n    ${lines.join('\n    ')}\n  };` : 'return {};';
    return `export function ${fnName}(\n  input: ${pascal}${label}Args\n): ${baseArgs} {\n  ${body}\n}`;
  };

  const initToBase = buildToBase('InitInfo', initFields);
  const updateToBase = buildToBase('UpdateInfo', updateFields);

  // ---- fromBase function ----
  const fromLines = [];
  baseFields.forEach((c) => {
    if (c.cat === 'sub') {
      useSubFrom(c.sub);
      fromLines.push(
        c.optional
          ? `${c.name}: input.${c.name}.__option === 'Some' ? ${c.sub.from}(input.${c.name}.value) : undefined,`
          : `${c.name}: ${c.sub.from}(input.${c.name}),`
      );
    } else if (c.cat === 'subArray') {
      useSubFrom(c.sub);
      fromLines.push(
        c.optional
          ? `${c.name}: input.${c.name}.__option === 'Some' ? input.${c.name}.value.map(${c.sub.from}) : undefined,`
          : `${c.name}: input.${c.name}.map(${c.sub.from}),`
      );
    }
  });
  if (ov.extraFromBase) {
    fromLines.push(ov.extraFromBase);
    (ov.extraFromBaseImports || []).forEach((i) => imp.use(i.mod, ...i.names));
  }
  if (ov.injectData) {
    imp.use('../../plugins/lib', 'parseExternalPluginAdapterData');
    fromLines.push('data: parseExternalPluginAdapterData(input, record, account),');
  }
  const fromBody =
    fromLines.length > 0
      ? `return {\n    ...input,\n    ${fromLines.join('\n    ')}\n  };`
      : 'return { ...input };';
  const fromBase = `export function ${camel}FromBase(\n  input: Base${pascal},\n  record: ExternalRegistryRecord,\n  account: Uint8Array\n): ${pascal} {\n  ${fromBody}\n}`;

  // ---- manifest ----
  imp.use('..', `Base${pascal}InitInfoArgs`, `Base${pascal}UpdateInfoArgs`);
  const manifest = `export const ${camel}Manifest: ExternalPluginAdapterManifest<
  ${pascal},
  Base${pascal},
  ${pascal}InitInfoArgs,
  Base${pascal}InitInfoArgs,
  ${pascal}UpdateInfoArgs,
  Base${pascal}UpdateInfoArgs
> = {
  type: '${pascal}',
  fromBase: ${camel}FromBase,
  initToBase: ${camel}InitInfoArgsToBase,
  updateToBase: ${camel}UpdateInfoArgsToBase,
};`;

  const code = [
    HEADER,
    imp.render(),
    '',
    dataType,
    '',
    pluginType,
    '',
    initType,
    '',
    updateType,
    '',
    initToBase,
    '',
    updateToBase,
    '',
    fromBase,
    '',
    manifest,
    '',
  ].join('\n');

  return code;
}

// ---------------------------------------------------------------------------
// base.ts (shared adapter types, kept value-free to avoid import cycles).
// ---------------------------------------------------------------------------
function generateBaseFile() {
  return `${HEADER}
import { BasePlugin } from '../../plugins/types';
import { LifecycleChecksContainer } from '../../plugins/lifecycleChecks';

export type ExternalPluginAdapterData = {
  dataLen?: bigint;
  dataOffset?: bigint;
};

export type BaseExternalPluginAdapter = BasePlugin &
  ExternalPluginAdapterData &
  LifecycleChecksContainer;
`;
}

// ---------------------------------------------------------------------------
// registry.ts (unions, manifests map, dispatch, init/update helpers).
// ---------------------------------------------------------------------------
function generateRegistryFile(adapters) {
  const imp = makeImports();
  imp.use('@metaplex-foundation/umi', 'isSome');
  imp.use(
    '..',
    'ExternalRegistryRecord',
    'getExternalPluginAdapterSerializer',
    'BaseExternalPluginAdapterInitInfoArgs',
    'BaseExternalPluginAdapterKey',
    'BaseExternalPluginAdapterUpdateInfoArgs'
  );
  imp.use('./base', 'BaseExternalPluginAdapter', 'ExternalPluginAdapterData');
  imp.use('../../plugins/pluginAuthority', 'pluginAuthorityFromBase');
  imp.use('../../plugins/lifecycleChecks', 'lifecycleChecksFromBase');

  adapters.forEach((a) => {
    imp.use(
      `./${a.camel}`,
      `${a.pascal}Plugin`,
      `${a.pascal}InitInfoArgs`,
      `${a.camel}Manifest`
    );
    if (a.updatable !== false) imp.use(`./${a.camel}`, `${a.pascal}UpdateInfoArgs`);
  });

  const typeString = `export type ExternalPluginAdapterTypeString =\n  BaseExternalPluginAdapterKey['__kind'];`;

  const dataReexport = `export type { BaseExternalPluginAdapter, ExternalPluginAdapterData };`;

  const unionAdapters = `export type ExternalPluginAdapters =\n  | ${adapters
    .map((a) => `${a.pascal}Plugin`)
    .join('\n  | ')};`;

  const listType = `export type ExternalPluginAdaptersList = {\n${adapters
    .map((a) => `  ${a.listKey}?: ${a.pascal}Plugin[];`)
    .join('\n')}\n};`;

  const initUnion = `export type ExternalPluginAdapterInitInfoArgs =\n  | ${adapters
    .map((a) => `({\n      type: '${a.pascal}';\n    } & ${a.pascal}InitInfoArgs)`)
    .join('\n  | ')};`;

  const updatable = adapters.filter((a) => a.updatable !== false);
  const updateUnion = `export type ExternalPluginAdapterUpdateInfoArgs =\n  | ${updatable
    .map((a) => `({\n      type: '${a.pascal}';\n    } & ${a.pascal}UpdateInfoArgs)`)
    .join('\n  | ')};`;

  const manifests = `export const externalPluginAdapterManifests = {\n${adapters
    .map((a) => `  ${a.pascal}: ${a.camel}Manifest,`)
    .join('\n')}\n};`;

  const meta = `const externalPluginAdapterMeta: Record<\n  ExternalPluginAdapterTypeString,\n  { listKey: keyof ExternalPluginAdaptersList; dataStore: boolean }\n> = {\n${adapters
    .map(
      (a) =>
        `  ${a.pascal}: { listKey: '${a.listKey}', dataStore: ${Boolean(
          a.injectData
        )} },`
    )
    .join('\n')}\n};`;

  const isType = `export const isExternalPluginAdapterType = (plugin: { type: string }) =>\n  plugin.type in externalPluginAdapterManifests;`;

  const createInit = `export function createExternalPluginAdapterInitInfo({\n  type,\n  ...args\n}: ExternalPluginAdapterInitInfoArgs): BaseExternalPluginAdapterInitInfoArgs {\n  const manifest = externalPluginAdapterManifests[type];\n  return {\n    __kind: type,\n    fields: [manifest.initToBase(args as any)] as any,\n  };\n}`;

  const createUpdate = `export function createExternalPluginAdapterUpdateInfo({\n  type,\n  ...args\n}: ExternalPluginAdapterUpdateInfoArgs): BaseExternalPluginAdapterUpdateInfoArgs {\n  const manifest = externalPluginAdapterManifests[type];\n  return {\n    __kind: type,\n    fields: [manifest.updateToBase(args as any)] as any,\n  };\n}`;

  const dispatch = `export function externalRegistryRecordsToExternalPluginAdapterList(
  records: ExternalRegistryRecord[],
  accountData: Uint8Array
): ExternalPluginAdaptersList {
  const result: ExternalPluginAdaptersList = {};

  records.forEach((record) => {
    const deserializedPlugin = getExternalPluginAdapterSerializer().deserialize(
      accountData,
      Number(record.offset)
    )[0];

    const base: BaseExternalPluginAdapter = {
      lifecycleChecks:
        record.lifecycleChecks.__option === 'Some'
          ? lifecycleChecksFromBase(record.lifecycleChecks.value)
          : undefined,
      authority: pluginAuthorityFromBase(record.authority),
      offset: record.offset,
    };

    const type = deserializedPlugin.__kind as ExternalPluginAdapterTypeString;
    const meta = externalPluginAdapterMeta[type];
    if (!meta) return;

    const dataFields: ExternalPluginAdapterData = meta.dataStore
      ? {
          dataOffset: isSome(record.dataOffset)
            ? record.dataOffset.value
            : undefined,
          dataLen: isSome(record.dataLen) ? record.dataLen.value : undefined,
        }
      : {};

    const manifest = externalPluginAdapterManifests[type];
    const list = (result[meta.listKey] ??= [] as any) as any[];
    list.push({
      type,
      ...dataFields,
      ...base,
      ...manifest.fromBase(deserializedPlugin.fields[0] as any, record, accountData),
    });
  });

  return result;
}`;

  const code = [
    HEADER,
    imp.render(),
    '',
    typeString,
    '',
    dataReexport,
    '',
    unionAdapters,
    '',
    listType,
    '',
    initUnion,
    '',
    updateUnion,
    '',
    manifests,
    '',
    meta,
    '',
    isType,
    '',
    createInit,
    '',
    createUpdate,
    '',
    dispatch,
    '',
  ].join('\n');

  return code;
}

function generateIndexFile(adapters) {
  // `base` is intentionally omitted: its two types are re-exported through
  // `registry` (the ergonomic replacement for the old externalPluginAdapters
  // module), so listing it here too would be a duplicate star-export.
  const files = [...adapters.map((a) => a.camel), 'registry'];
  return `${HEADER}\n${files.map((f) => `export * from './${f}';`).join('\n')}\n`;
}

// ---------------------------------------------------------------------------
// Entry point: build the render map from the root node.
// ---------------------------------------------------------------------------
function buildAdapters(root) {
  const program = root.programs[0];
  const dtByName = new Map();
  program.definedTypes.forEach((dt) => dtByName.set(String(dt.name), dt));

  const enumNode = dtByName.get('externalPluginAdapter');
  if (!enumNode || enumNode.type.kind !== 'enumTypeNode') {
    throw new Error('Could not find `externalPluginAdapter` enum in the IDL.');
  }

  return enumNode.type.variants.map((variant) => {
    const pascal = cap(String(variant.name));
    const camel = low(pascal);
    const baseName = `base${pascal}`;
    const ov = OVERRIDES[pascal] || {};

    const fieldsOf = (node) => {
      if (!node) return [];
      if (node.type.kind !== 'structTypeNode') return [];
      return node.type.fields.map(classify);
    };

    return {
      pascal,
      camel,
      baseName,
      ov,
      listKey: pluralize(pascal),
      injectData: Boolean(ov.injectData),
      updatable: ov.updatable,
      baseFields: fieldsOf(dtByName.get(baseName)),
      initFields: fieldsOf(dtByName.get(`${baseName}InitInfo`)),
      updateFields: fieldsOf(dtByName.get(`${baseName}UpdateInfo`)),
    };
  });
}

function buildRenderMap(root) {
  const adapters = buildAdapters(root);
  const files = {};
  files['base.ts'] = generateBaseFile();
  adapters.forEach((a) => {
    files[`${a.camel}.ts`] = generateAdapterFile(a);
  });
  files['registry.ts'] = generateRegistryFile(adapters);
  files['index.ts'] = generateIndexFile(adapters);
  return files;
}

async function generateExternalPluginAdapters(root, jsGeneratedDir, prettierConfig) {
  const path = require('path');
  const fs = require('fs');
  const prettier = require('prettier');
  const outDir = path.join(jsGeneratedDir, 'plugins');
  fs.mkdirSync(outDir, { recursive: true });
  const files = buildRenderMap(root);
  for (const [rel, raw] of Object.entries(files)) {
    // eslint-disable-next-line no-await-in-loop
    const formatted = await prettier.format(raw, {
      ...prettierConfig,
      parser: 'typescript',
    });
    fs.writeFileSync(path.join(outDir, rel), formatted);
  }
  return Object.keys(files).map((f) => path.join('plugins', f));
}

module.exports = {
  buildRenderMap,
  buildAdapters,
  generateExternalPluginAdapters,
};
