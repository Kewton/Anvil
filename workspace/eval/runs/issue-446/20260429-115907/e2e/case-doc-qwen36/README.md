# Widget CLI

## Usage

```bash
widget-cli run --config config.yaml
```

Widget CLI helps operators inspect local widget reports.

## Usage

```bash
widget-cli inspect --report ./reports/widget-report.json
widget-cli --help
```

## Troubleshooting

| エラー | 対処例 |
|--------|--------|
| `Error: ENOENT: no such file or directory` | `--report` に指定したパスが正しいか確認し、ファイルが存在することを確認してください。例: `ls ./reports/widget-report.json` |
