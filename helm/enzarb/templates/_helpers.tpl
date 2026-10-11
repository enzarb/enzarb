{{/* Selector labels. Usage: include "enzarb.selector" (dict "ctx" . "name" "enzarb-ui") */}}
{{- define "enzarb.selector" -}}
app.kubernetes.io/name: {{ .name }}
app.kubernetes.io/instance: {{ .ctx.Release.Name }}
{{- end }}

{{/* Common labels. app.kubernetes.io/name must equal the component name: the operator's
     sandbox NetworkPolicy matches enzarb-sandbox-proxy and enzarb-workflows by it. */}}
{{- define "enzarb.labels" -}}
{{ include "enzarb.selector" . }}
app.kubernetes.io/part-of: enzarb
app.kubernetes.io/managed-by: {{ .ctx.Release.Service }}
helm.sh/chart: {{ printf "%s-%s" .ctx.Chart.Name .ctx.Chart.Version | replace "+" "_" }}
{{- end }}

{{/* image reference. Usage: include "enzarb.image" .Values.ui */}}
{{- define "enzarb.image" -}}
{{ .image.repository }}:{{ .image.tag }}
{{- end }}

{{/* pull policy. Usage: include "enzarb.pullPolicy" (dict "ctx" . "c" .Values.ui) */}}
{{- define "enzarb.pullPolicy" -}}
{{ .c.image.pullPolicy | default .ctx.Values.imagePullPolicy }}
{{- end }}

{{- define "enzarb.pullSecrets" -}}
{{- with .Values.imagePullSecrets }}
imagePullSecrets:
  {{- toYaml . | nindent 2 }}
{{- end }}
{{- end }}

{{- define "enzarb.publicUrl" -}}
{{ .Values.publicUrl | default (printf "https://%s" .Values.host) }}
{{- end }}

{{/* Secret names */}}
{{- define "enzarb.generatedSecret" -}}enzarb-generated{{- end }}
{{- define "enzarb.githubSecret" -}}{{ .Values.secrets.github.existingSecret | default "enzarb-github" }}{{- end }}
{{- define "enzarb.jwtSecret" -}}{{ .Values.secrets.sessionJwt.existingSecret | default (include "enzarb.generatedSecret" .) }}{{- end }}
{{- define "enzarb.callbackSecret" -}}{{ .Values.secrets.executorCallback.existingSecret | default (include "enzarb.generatedSecret" .) }}{{- end }}
{{- define "enzarb.jwtKey" -}}{{ if .Values.secrets.sessionJwt.existingSecret }}{{ .Values.secrets.sessionJwt.key }}{{ else }}session-jwt-secret{{ end }}{{- end }}
{{- define "enzarb.callbackKey" -}}{{ if .Values.secrets.executorCallback.existingSecret }}{{ .Values.secrets.executorCallback.key }}{{ else }}callback-secret{{ end }}{{- end }}
{{/* Key names inside the github Secret (the chart-created one uses the configured key names too). */}}
{{- define "enzarb.githubKey" -}}{{ index .ctx.Values.secrets.github.keys .key }}{{- end }}
{{/* enzarb-updater Secret: an existing one (keys from secrets.updater.keys) or the chart-created one. */}}
{{- define "enzarb.updaterSecret" -}}{{ .Values.secrets.updater.existingSecret | default "enzarb-updater" }}{{- end }}
{{- define "enzarb.updaterKey" -}}
{{- if .ctx.Values.secrets.updater.existingSecret }}{{ index .ctx.Values.secrets.updater.keys .key }}{{ else if eq .key "webhookSecret" }}updater-webhook-secret{{ else }}updater-registry-token{{ end }}
{{- end }}
{{/* Non-empty when ENZARB_UPDATER_REGISTRY_TOKEN is wired (from any source). */}}
{{- define "enzarb.updaterHasRegistryToken" -}}
{{- $s := .Values.secrets.updater }}
{{- if or $s.registryTokenSecret $s.registryToken (and $s.existingSecret $s.hasRegistryToken) }}true{{ end }}
{{- end }}
{{/* Secret name and key of the registry token. */}}
{{- define "enzarb.updaterTokenSecret" -}}{{ .Values.secrets.updater.registryTokenSecret | default (include "enzarb.updaterSecret" .) }}{{- end }}
{{- define "enzarb.updaterTokenKey" -}}
{{- if .Values.secrets.updater.registryTokenSecret }}{{ .Values.secrets.updater.registryTokenKey }}{{ else }}{{ include "enzarb.updaterKey" (dict "ctx" . "key" "registryToken") }}{{ end }}
{{- end }}

{{/* secretKeyRef env var. Usage: include "enzarb.secretEnv" (dict "name" "X" "secret" "s" "key" "k") */}}
{{- define "enzarb.secretEnv" -}}
- name: {{ .name }}
  valueFrom:
    secretKeyRef:
      name: {{ .secret }}
      key: {{ .key }}
{{- end }}

{{/*
Database connection env for a pgop Role, from the Role credentials Secret
`<cluster>-<role>-credentials` (keys username, password, host, port, sslmode).
The credentials are passed as libpq-style PG* variables, never spliced into a URL, so a
password with URL-special characters cannot break the connection string. DATABASE_URL
only names the database: `postgres:///enzarb` (no host, user or password), and sqlx
(PgConnectOptions::from_str) falls back to PGHOST/PGPORT/PGUSER/PGPASSWORD/PGSSLMODE.
postgres.js reads PGHOST/PGPORT/PGUSER/PGPASSWORD for missing URL parts but NOT
PGSSLMODE (it reads PGSSL), so with "urlSslmode" true the sslmode (not a secret, always
URL-safe) goes into the URL as `?sslmode=$(PGSSLMODE)`.
pgop's Role Secret `uri` points at the `postgres` database, hence PGDATABASE from values.
Usage: include "enzarb.dbEnv" (dict "ctx" . "role" "workflows" "urlSslmode" false)
*/}}
{{- define "enzarb.dbEnv" -}}
{{- $s := printf "%s-%s-credentials" .ctx.Values.postgres.clusterName .role }}
{{- range $k, $v := dict "PGUSER" "username" "PGPASSWORD" "password" "PGHOST" "host" "PGPORT" "port" "PGSSLMODE" "sslmode" }}
{{ include "enzarb.secretEnv" (dict "name" $k "secret" $s "key" $v) }}
{{- end }}
- name: PGDATABASE
  value: {{ .ctx.Values.postgres.databaseName | quote }}
- name: DATABASE_URL
  value: {{ printf "postgres:///%s%s" .ctx.Values.postgres.databaseName (ternary "?sslmode=$(PGSSLMODE)" "" (default false .urlSslmode)) | quote }}
{{- end }}

{{/*
Name of a component's migration Job: enzarb-migrate-<comp>-<hash>. Job spec.template is
immutable, so the hash covers what shapes the rendered Job: image and pull policy, DB and
cluster names, wait image and timeout, pull secrets, securityContext, log level, the
upstream Job it waits for, and a template revision ("rev"; bump it when migrations.yaml or
the helpers change the Job shape). Any change gives a new Job; unchanged inputs keep the
completed Job.
Usage: include "enzarb.migrateJob" (dict "ctx" . "comp" "ui")
*/}}
{{- define "enzarb.migrateJob" -}}
{{- $ctx := .ctx -}}
{{- $v := $ctx.Values -}}
{{- $c := index $v .comp -}}
{{- $after := "" -}}
{{- if eq .comp "connectors" }}{{- $after = include "enzarb.migrateJob" (dict "ctx" $ctx "comp" "workflows") }}{{- end -}}
{{- if eq .comp "ui" }}{{- $after = include "enzarb.migrateJob" (dict "ctx" $ctx "comp" "connectors") }}{{- end -}}
{{- $spec := dict "rev" 2 "comp" .comp "after" $after "image" (include "enzarb.image" $c) "pull" (include "enzarb.pullPolicy" (dict "ctx" $ctx "c" $c)) "cluster" $v.postgres.clusterName "db" $v.postgres.databaseName "waitImage" $v.waitImage "waitTimeout" $v.waitTimeout "pullSecrets" $v.imagePullSecrets "sc" $v.securityContext "rustLog" $v.rustLog -}}
enzarb-migrate-{{ .comp }}-{{ toJson $spec | sha256sum | trunc 10 }}
{{- end }}

{{/* The last Job in the chain (workflows -> connectors -> ui); runtime pods wait for it. */}}
{{- define "enzarb.lastMigrateJob" -}}
{{ include "enzarb.migrateJob" (dict "ctx" . "comp" "ui") }}
{{- end }}

{{/* initContainer: wait for a Job. Usage: include "enzarb.waitJob" (dict "ctx" . "job" "name") */}}
{{- define "enzarb.waitJob" -}}
- name: wait-{{ .job | trunc 40 | trimSuffix "-" }}
  image: {{ include "enzarb.image" (dict "image" .ctx.Values.waitImage) }}
  imagePullPolicy: IfNotPresent
  args: [wait, --for=condition=complete, --timeout={{ .ctx.Values.waitTimeout }}, job/{{ .job }}]
  securityContext: {{ include "enzarb.securityContext" (dict "ctx" .ctx "uid" 65532) | fromYaml | toJson }}
{{- end }}

{{/* initContainer: wait for the pgop Cluster, Database and all Roles to be ready. */}}
{{- define "enzarb.waitPg" -}}
- name: wait-pgop
  image: {{ include "enzarb.image" (dict "image" .Values.waitImage) }}
  imagePullPolicy: IfNotPresent
  args:
    - wait
    - --for=jsonpath={.status.ready}=true
    - --timeout={{ .Values.waitTimeout }}
    - cluster.pgop.ruck.io/{{ .Values.postgres.clusterName }}
    - database.pgop.ruck.io/{{ .Values.postgres.databaseName }}
    {{- range tuple "workflows-owner" "workflows" "connectors-owner" "connectors" "ui-owner" "ui" }}
    - role.pgop.ruck.io/{{ . }}
    {{- end }}
  securityContext: {{ include "enzarb.securityContext" (dict "ctx" . "uid" 65532) | fromYaml | toJson }}
{{- end }}

{{/* Init containers that gate runtime pods on the finished migration chain. */}}
{{- define "enzarb.waitMigrations" -}}
{{- if .Values.postgres.enabled }}
initContainers:
  {{- include "enzarb.waitJob" (dict "ctx" . "job" (include "enzarb.lastMigrateJob" .)) | nindent 2 }}
{{- end }}
{{- end }}

{{/* container securityContext. Usage: include "enzarb.securityContext" (dict "ctx" . "uid" 65532) */}}
{{- define "enzarb.securityContext" -}}
{{- $sc := deepCopy .ctx.Values.securityContext -}}
{{- $_ := set $sc "runAsNonRoot" true -}}
{{- $_ := set $sc "runAsUser" .uid -}}
{{- $_ := set $sc "seccompProfile" (dict "type" "RuntimeDefault") -}}
{{- toYaml $sc -}}
{{- end }}
