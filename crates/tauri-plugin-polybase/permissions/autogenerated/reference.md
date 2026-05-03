## Default Permission

Default permission set for the polybase plugin.

Grants every command on the plugin's surface — configure, session management, edge function
calls, encryption helpers, KVS read/write/delete, and the full storage surface (upload,
download, delete, list, signed URL).

Apps that want a tighter permission boundary can drop `polybase:default` and pick individual
`polybase:allow-*` entries instead.

#### This default permission set includes the following:

- `allow-configure`
- `allow-set-session`
- `allow-clear-session`
- `allow-current-session`
- `allow-edge-call`
- `allow-encrypt`
- `allow-decrypt`
- `allow-encrypt-batch`
- `allow-decrypt-batch`
- `allow-kvs-get`
- `allow-kvs-set`
- `allow-kvs-delete`
- `allow-storage-upload`
- `allow-storage-download`
- `allow-storage-delete`
- `allow-storage-list`
- `allow-storage-signed-url`

## Permission Table

<table>
<tr>
<th>Identifier</th>
<th>Description</th>
</tr>


<tr>
<td>

`polybase:allow-clear-session`

</td>
<td>

Enables the clear_session command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:deny-clear-session`

</td>
<td>

Denies the clear_session command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:allow-configure`

</td>
<td>

Enables the configure command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:deny-configure`

</td>
<td>

Denies the configure command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:allow-current-session`

</td>
<td>

Enables the current_session command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:deny-current-session`

</td>
<td>

Denies the current_session command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:allow-decrypt`

</td>
<td>

Enables the decrypt command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:deny-decrypt`

</td>
<td>

Denies the decrypt command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:allow-decrypt-batch`

</td>
<td>

Enables the decrypt_batch command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:deny-decrypt-batch`

</td>
<td>

Denies the decrypt_batch command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:allow-edge-call`

</td>
<td>

Enables the edge_call command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:deny-edge-call`

</td>
<td>

Denies the edge_call command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:allow-encrypt`

</td>
<td>

Enables the encrypt command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:deny-encrypt`

</td>
<td>

Denies the encrypt command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:allow-encrypt-batch`

</td>
<td>

Enables the encrypt_batch command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:deny-encrypt-batch`

</td>
<td>

Denies the encrypt_batch command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:allow-kvs-delete`

</td>
<td>

Enables the kvs_delete command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:deny-kvs-delete`

</td>
<td>

Denies the kvs_delete command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:allow-kvs-get`

</td>
<td>

Enables the kvs_get command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:deny-kvs-get`

</td>
<td>

Denies the kvs_get command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:allow-kvs-set`

</td>
<td>

Enables the kvs_set command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:deny-kvs-set`

</td>
<td>

Denies the kvs_set command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:allow-set-session`

</td>
<td>

Enables the set_session command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:deny-set-session`

</td>
<td>

Denies the set_session command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:allow-storage-delete`

</td>
<td>

Enables the storage_delete command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:deny-storage-delete`

</td>
<td>

Denies the storage_delete command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:allow-storage-download`

</td>
<td>

Enables the storage_download command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:deny-storage-download`

</td>
<td>

Denies the storage_download command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:allow-storage-list`

</td>
<td>

Enables the storage_list command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:deny-storage-list`

</td>
<td>

Denies the storage_list command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:allow-storage-signed-url`

</td>
<td>

Enables the storage_signed_url command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:deny-storage-signed-url`

</td>
<td>

Denies the storage_signed_url command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:allow-storage-upload`

</td>
<td>

Enables the storage_upload command without any pre-configured scope.

</td>
</tr>

<tr>
<td>

`polybase:deny-storage-upload`

</td>
<td>

Denies the storage_upload command without any pre-configured scope.

</td>
</tr>
</table>
