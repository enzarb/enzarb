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

{{/* secretKeyRef env var. Usage: include "enzarb.secretEnv" (dict "name" "X" "secret" "s" "key" "k") */}}
{{- define "enzarb.secretEnv" -}}
- name: {{ .name }}
  valueFrom:
    secretKeyRef:
      name: {{ .secret }}
      key: {{ .key }}
{{- end }}

{{/*
DATABASE_URL for a pgop Role, built from the Role credentials Secret
`<cluster>-<role>-credentials` (keys username, password, host, port, sslmode).
pgop only creates the per-database Secret `<database>-<owner>-credentials` for the
Database owner, and the Role Secret's own `uri` points at the `postgres` database, so
the URL is assembled here. Usage: include "enzarb.dbEnv" (dict "ctx" . "role" "workflows")
*/}}
{{- define "enzarb.dbEnv" -}}
{{- $s := printf "%s-%s-credentials" .ctx.Values.postgres.clusterName .role }}
{{- range $k, $v := dict "ENZARB_DB_USER" "username" "ENZARB_DB_PASSWORD" "password" "ENZARB_DB_HOST" "host" "ENZARB_DB_PORT" "port" "ENZARB_DB_SSLMODE" "sslmode" }}
{{ include "enzarb.secretEnv" (dict "name" $k "secret" $s "key" $v) }}
{{- end }}
- name: DATABASE_URL
  value: {{ printf "postgres://$(ENZARB_DB_USER):$(ENZARB_DB_PASSWORD)@$(ENZARB_DB_HOST):$(ENZARB_DB_PORT)/%s?sslmode=$(ENZARB_DB_SSLMODE)" .ctx.Values.postgres.databaseName | quote }}
{{- end }}

{{/*
Name of a component's migration Job. The suffix hashes the image, so a new image gets a
new (immutable) Job and an unchanged image keeps its completed Job.
Usage: include "enzarb.migrateJob" (dict "c" .Values.ui "comp" "ui")
*/}}
{{- define "enzarb.migrateJob" -}}
enzarb-migrate-{{ .comp }}-{{ include "enzarb.image" .c | sha256sum | trunc 8 }}
{{- end }}

{{/* The last Job in the chain (workflows -> connectors -> ui); runtime pods wait for it. */}}
{{- define "enzarb.lastMigrateJob" -}}
{{ include "enzarb.migrateJob" (dict "c" .Values.ui "comp" "ui") }}
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
