{{/*
_helpers.tpl — shared template fragments for the Ravn agents chart.
*/}}

{{/*
Expand the chart name, capped at 63 chars (Kubernetes label value limit).
*/}}
{{- define "ravn.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Full release-scoped name, used for ClusterRole / ClusterRoleBinding names
where release isolation matters.
*/}}
{{- define "ravn.fullname" -}}
{{- $name := default .Chart.Name .Values.nameOverride }}
{{- if contains $name .Release.Name }}
{{- .Release.Name | trunc 63 | trimSuffix "-" }}
{{- else }}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" }}
{{- end }}
{{- end }}

{{/*
Chart version label value.
*/}}
{{- define "ravn.chart" -}}
{{- printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end }}

{{/*
Recommended labels — applied to every object managed by this chart.
Helm's standard label set; do not remove `helm.sh/chart` or `app.kubernetes.io/managed-by`.
*/}}
{{- define "ravn.labels" -}}
helm.sh/chart: {{ include "ravn.chart" . }}
app.kubernetes.io/name: {{ include "ravn.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: ravn
{{- end }}

{{/*
Selector labels — the minimal stable set used in matchLabels / Service selectors.
Never include version or chart here; those change across upgrades and would break
rolling updates.
*/}}
{{- define "ravn.selectorLabels" -}}
app.kubernetes.io/name: {{ include "ravn.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end }}

{{/*
Component-scoped selector labels.
Usage: {{ include "ravn.componentSelectorLabels" (list . "controller") }}
*/}}
{{- define "ravn.componentSelectorLabels" -}}
{{- $ctx := index . 0 -}}
{{- $component := index . 1 -}}
{{ include "ravn.selectorLabels" $ctx }}
app.kubernetes.io/component: {{ $component }}
{{- end }}

{{/*
The projected, audience-bound ServiceAccount token volume.
Mounts the kubelet-rotated token at <tokenMount>/token so both agents and (once
#146 ships) the executor can use it to authenticate to the control plane.
*/}}
{{- define "ravn.tokenVolume" -}}
- name: ravn-token
  projected:
    sources:
      - serviceAccountToken:
          path: token
          audience: {{ .Values.controlPlane.tokenAudience | quote }}
          expirationSeconds: 3600
{{- end }}

{{/*
Standard volume mount for the projected token.
*/}}
{{- define "ravn.tokenVolumeMount" -}}
- name: ravn-token
  mountPath: {{ .Values.controlPlane.tokenMount | quote }}
  readOnly: true
{{- end }}

{{/*
Common environment variables shared by controller, node-agent, and (once #146
ships) the executor.
*/}}
{{- define "ravn.commonEnv" -}}
- name: RAVN_INGEST_URL
  value: {{ .Values.controlPlane.ingestUrl | quote }}
- name: RAVN_SA_TOKEN_FILE
  value: {{ printf "%s/token" .Values.controlPlane.tokenMount | quote }}
- name: RAVN_CLUSTER
  value: {{ .Values.cluster | quote }}
- name: RAVN_LOG
  value: {{ .Values.logLevel | quote }}
{{- end }}

{{/*
Resolve the effective image for a component. Accepts a component-level image
override (e.g. .Values.executor.image); falls back to the top-level image.
Usage: {{ include "ravn.image" (list . .Values.executor.image) }}
*/}}
{{- define "ravn.image" -}}
{{- $ctx := index . 0 -}}
{{- $override := index . 1 -}}
{{- $repo := $override.repository | default $ctx.Values.image.repository -}}
{{- $tag  := $override.tag        | default $ctx.Values.image.tag -}}
{{- printf "%s:%s" $repo $tag }}
{{- end }}
