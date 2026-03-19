import { signal } from "@preact/signals";
import { html } from "htm/preact";
import { render } from "preact";
import { useEffect, useRef } from "preact/hooks";

var snapshot = signal(null);
var loading = signal(false);
var error = signal("");
var selectedId = signal("");
var selectedDetail = signal(null);
var detailLoading = signal(false);
var filterNodeType = signal("all");
var filterStatus = signal("all");
var imaginedOnly = signal(false);
var neighborhoodOnly = signal(false);
var searchQuery = signal("");
var hideWeakNodes = signal(false);
var graphZoom = signal(1);
var graphPanX = signal(0);
var graphPanY = signal(0);
var containerRef = null;

async function fetchJson(url) {
	var res = await fetch(url, {
		headers: { accept: "application/json" },
		credentials: "same-origin",
	});
	var data = await res.json().catch(() => ({}));
	if (!res.ok) throw new Error(data?.error || `Request failed (${res.status})`);
	return data;
}

async function refreshSnapshot() {
	loading.value = true;
	error.value = "";
	try {
		var data = await fetchJson("/api/nodamem/graph");
		snapshot.value = data;
		if (!selectedId.value && data?.nodes?.length) {
			selectedId.value = data.nodes[0].id;
		}
	} catch (err) {
		error.value = err?.message || "Failed to load Nodamem graph";
		snapshot.value = null;
		selectedId.value = "";
		selectedDetail.value = null;
	} finally {
		loading.value = false;
	}
}

function titleFor(item) {
	if (!item) return "";
	return item.title || item.label || item.id;
}

function summaryFor(item) {
	if (!item) return "";
	return item.summary || item.statement || item.description || item.premise || "";
}

function compact(text, max) {
	var value = String(text || "").replace(/\s+/g, " ").trim();
	if (value.length <= max) return value;
	return `${value.slice(0, Math.max(0, max - 1)).trimEnd()}...`;
}

function kindLabel(item) {
	if (item.kind === "imagined") return "imagined";
	if (item.kind === "lesson") return "lesson";
	if (item.kind === "checkpoint") return "checkpoint";
	if (item.kind === "trait") return "trait";
	return item.node_type || "node";
}

function statusBucket(item) {
	if (item.kind === "imagined") return "imagined";
	return item.status === "archived" || item.status === "pruned" ? "archived" : "active";
}

function scoreFor(item, key) {
	var value = Number(item?.[key]);
	return Number.isFinite(value) ? value : null;
}

function isWeakNode(item) {
	if (item.kind !== "node") return false;
	var confidence = scoreFor(item, "confidence");
	var importance = scoreFor(item, "importance");
	if (confidence === null && importance === null) return false;
	return (confidence ?? 1) < 0.45 && (importance ?? 1) < 0.35;
}

function matchesSearch(item) {
	var query = searchQuery.value.trim().toLowerCase();
	if (!query) return true;
	var haystack = [
		item.id,
		titleFor(item),
		item.label,
		kindLabel(item),
		item.node_type,
		item.status,
		summaryFor(item),
	]
		.filter(Boolean)
		.join(" ")
		.toLowerCase();
	return haystack.includes(query);
}

function isVisibleItem(item, allowedIds) {
	if (allowedIds && !allowedIds.has(item.id)) return false;
	if (!matchesSearch(item)) return false;
	if (hideWeakNodes.value && isWeakNode(item)) return false;
	if (imaginedOnly.value) return item.kind === "imagined";
	if (filterStatus.value === "active" && statusBucket(item) !== "active") return false;
	if (filterStatus.value === "archived" && statusBucket(item) !== "archived") return false;
	if (filterStatus.value === "imagined" && item.kind !== "imagined") return false;
	if (filterNodeType.value !== "all" && item.kind === "node" && item.node_type !== filterNodeType.value) {
		return false;
	}
	return true;
}

function buildItems(data) {
	var nodes = (data?.nodes || []).map((node) => ({
		...node,
		kind: "node",
		label: titleFor(node) || compact(node.summary, 32) || node.id,
	}));
	var lessons = (data?.lessons || []).map((lesson) => ({
		...lesson,
		id: `lesson:${lesson.id}`,
		kind: "lesson",
		label: lesson.title || "Lesson",
	}));
	var checkpoints = (data?.checkpoints || []).map((checkpoint) => ({
		...checkpoint,
		id: `checkpoint:${checkpoint.id}`,
		kind: "checkpoint",
		label: checkpoint.title || "Checkpoint",
	}));
	var traits = (data?.traits || []).map((trait) => ({
		...trait,
		id: `trait:${trait.id}`,
		kind: "trait",
		label: trait.label || "Trait",
	}));
	var imagined = (data?.imagined_scenarios || []).map((scenario) => ({
		...scenario,
		id: `imagined:${scenario.id}`,
		kind: "imagined",
		label: scenario.title || "Imagined scenario",
	}));
	return nodes.concat(lessons, checkpoints, traits, imagined);
}

function buildEdges(data) {
	var base = (data?.edges || []).map((edge) => ({
		id: edge.id,
		from: edge.from_node_id,
		to: edge.to_node_id,
		kind: edge.edge_type || "related_to",
	}));
	var lessonEdges = (data?.lessons || []).flatMap((lesson) =>
		(lesson.supporting_node_ids || []).map((nodeId) => ({
			id: `lesson-support:${lesson.id}:${nodeId}`,
			from: `lesson:${lesson.id}`,
			to: nodeId,
			kind: "supports",
		})).concat(
			(lesson.contradicting_node_ids || []).map((nodeId) => ({
				id: `lesson-contradict:${lesson.id}:${nodeId}`,
				from: `lesson:${lesson.id}`,
				to: nodeId,
				kind: "contradicts",
			})),
		),
	);
	var checkpointEdges = (data?.checkpoints || []).flatMap((checkpoint) =>
		(checkpoint.node_ids || []).map((nodeId) => ({
			id: `checkpoint-node:${checkpoint.id}:${nodeId}`,
			from: `checkpoint:${checkpoint.id}`,
			to: nodeId,
			kind: "checkpoint",
		})),
	);
	var traitEdges = (data?.traits || []).flatMap((trait) =>
		(trait.supporting_node_ids || []).map((nodeId) => ({
			id: `trait-node:${trait.id}:${nodeId}`,
			from: `trait:${trait.id}`,
			to: nodeId,
			kind: "trait",
		})).concat(
			(trait.supporting_lesson_ids || []).map((lessonId) => ({
				id: `trait-lesson:${trait.id}:${lessonId}`,
				from: `trait:${trait.id}`,
				to: `lesson:${lessonId}`,
				kind: "trait",
			})),
		),
	);
	var imaginedEdges = (data?.imagined_scenarios || []).flatMap((scenario) =>
		(scenario.basis_source_node_ids || []).map((nodeId) => ({
			id: `imagined-basis:${scenario.id}:${nodeId}`,
			from: `imagined:${scenario.id}`,
			to: nodeId,
			kind: "imagined",
		})),
	);
	return base.concat(lessonEdges, checkpointEdges, traitEdges, imaginedEdges);
}

function buildNeighborhood(edges, focusId) {
	if (!focusId) return null;
	var allowed = new Set([focusId]);
	edges.forEach((edge) => {
		if (edge.from === focusId) allowed.add(edge.to);
		if (edge.to === focusId) allowed.add(edge.from);
	});
	return allowed;
}

function visibleGraph(data) {
	var items = buildItems(data);
	var edges = buildEdges(data);
	var allowed = neighborhoodOnly.value ? buildNeighborhood(edges, selectedId.value) : null;
	var visibleItems = items.filter((item) => isVisibleItem(item, allowed));
	var visibleIds = new Set(visibleItems.map((item) => item.id));
	var visibleEdges = edges.filter((edge) => visibleIds.has(edge.from) && visibleIds.has(edge.to));
	return { items: visibleItems, edges: visibleEdges };
}

function laneFor(item) {
	if (item.kind === "imagined") return 4;
	if (item.kind === "trait") return 3;
	if (item.kind === "lesson" || item.kind === "checkpoint") return 2;
	if (item.kind === "node" && (item.node_type === "goal" || item.node_type === "preference")) return 0;
	return 1;
}

function compareItems(a, b) {
	if (statusBucket(a) !== statusBucket(b)) return statusBucket(a) === "active" ? -1 : 1;
	var aScore = (scoreFor(a, "importance") ?? scoreFor(a, "confidence") ?? scoreFor(a, "strength") ?? 0);
	var bScore = (scoreFor(b, "importance") ?? scoreFor(b, "confidence") ?? scoreFor(b, "strength") ?? 0);
	if (aScore !== bScore) return bScore - aScore;
	return titleFor(a).localeCompare(titleFor(b));
}

function layoutGraph(graph) {
	var laneWidth = 280;
	var laneStart = 90;
	var rowGap = 94;
	var clusterGap = 28;
	var nodeWidth = 184;
	var nodeHeight = 54;
	var lanes = [[], [], [], [], []];
	graph.items.forEach((item) => lanes[laneFor(item)].push(item));
	lanes.forEach((laneItems) => laneItems.sort(compareItems));

	var positions = {};
	lanes.forEach((laneItems, laneIndex) => {
		laneItems.forEach((item, rowIndex) => {
			var clusterOffset = Math.floor(rowIndex / 6) * clusterGap;
			positions[item.id] = {
				x: laneStart + laneIndex * laneWidth,
				y: 70 + rowIndex * rowGap + clusterOffset,
			};
		});
	});

	var contentWidth = laneStart * 2 + laneWidth * lanes.length;
	var contentHeight = Math.max(460, ...Object.values(positions).map((p) => p.y + nodeHeight + 80), 460);
	return { positions, laneWidth, nodeWidth, nodeHeight, width: contentWidth, height: contentHeight };
}

function nodeStyle(item) {
	if (item.kind === "imagined") {
		return { fill: "rgba(245,158,11,0.14)", stroke: "#d97706", text: "var(--text-strong)", accent: "rgba(245,158,11,0.24)" };
	}
	if (statusBucket(item) === "archived") {
		return { fill: "rgba(113,113,122,0.1)", stroke: "rgba(113,113,122,0.72)", text: "var(--text)", accent: "rgba(113,113,122,0.18)" };
	}
	if (item.kind === "trait") {
		return { fill: "rgba(14,165,233,0.12)", stroke: "#0284c7", text: "var(--text-strong)", accent: "rgba(14,165,233,0.18)" };
	}
	if (item.kind === "lesson") {
		return { fill: "rgba(59,130,246,0.12)", stroke: "#2563eb", text: "var(--text-strong)", accent: "rgba(59,130,246,0.18)" };
	}
	if (item.kind === "checkpoint") {
		return { fill: "rgba(20,184,166,0.12)", stroke: "#0f766e", text: "var(--text-strong)", accent: "rgba(20,184,166,0.18)" };
	}
	return { fill: "rgba(74,222,128,0.12)", stroke: "#16a34a", text: "var(--text-strong)", accent: "rgba(74,222,128,0.18)" };
}

function centerOnNode(nodeId, layout, viewportWidth, viewportHeight) {
	var pos = layout?.positions?.[nodeId];
	if (!pos) return;
	var scaledWidth = layout.nodeWidth * graphZoom.value;
	var scaledHeight = layout.nodeHeight * graphZoom.value;
	graphPanX.value = viewportWidth / 2 - (pos.x * graphZoom.value + scaledWidth / 2);
	graphPanY.value = viewportHeight / 2 - (pos.y * graphZoom.value + scaledHeight / 2);
}

function GraphCanvas() {
	var data = snapshot.value;
	var graph = visibleGraph(data);
	var layout = layoutGraph(graph);
	var viewportWidth = 1280;
	var viewportHeight = 620;
	var dragRef = useRef(null);

	function onNodeSelect(item) {
		selectedId.value = item.id;
		centerOnNode(item.id, layout, viewportWidth, viewportHeight);
	}

	function onWheel(event) {
		event.preventDefault();
		var nextZoom = graphZoom.value + (event.deltaY < 0 ? 0.12 : -0.12);
		graphZoom.value = Math.max(0.55, Math.min(2.1, Number(nextZoom.toFixed(2))));
	}

	function onPointerDown(event) {
		dragRef.current = {
			x: event.clientX,
			y: event.clientY,
			panX: graphPanX.value,
			panY: graphPanY.value,
		};
	}

	function onPointerMove(event) {
		if (!dragRef.current) return;
		graphPanX.value = dragRef.current.panX + (event.clientX - dragRef.current.x);
		graphPanY.value = dragRef.current.panY + (event.clientY - dragRef.current.y);
	}

	function stopDragging() {
		dragRef.current = null;
	}

	function resetView() {
		graphZoom.value = 1;
		graphPanX.value = 0;
		graphPanY.value = 0;
	}

	return html`<div class="rounded-lg border border-[var(--border)] bg-[var(--surface)] overflow-hidden">
		<div class="px-4 py-3 border-b border-[var(--border)] text-sm text-[var(--text-muted)] flex flex-wrap items-center gap-2 justify-between">
			<div>Developer graph view. Amber is hypothetical. Muted or dashed nodes are archived or superseded.</div>
			<div class="flex items-center gap-2">
				<button class="provider-btn provider-btn-secondary provider-btn-sm" onClick=${() => (graphZoom.value = Math.max(0.55, Number((graphZoom.value - 0.1).toFixed(2))))}>-</button>
				<div class="min-w-[58px] text-center text-xs text-[var(--muted)]">${Math.round(graphZoom.value * 100)}%</div>
				<button class="provider-btn provider-btn-secondary provider-btn-sm" onClick=${() => (graphZoom.value = Math.min(2.1, Number((graphZoom.value + 0.1).toFixed(2))))}>+</button>
				<button class="provider-btn provider-btn-secondary provider-btn-sm" onClick=${resetView}>Reset view</button>
				<button
					class="provider-btn provider-btn-secondary provider-btn-sm"
					disabled=${!selectedId.value}
					onClick=${() => centerOnNode(selectedId.value, layout, viewportWidth, viewportHeight)}
				>
					Focus selected
				</button>
			</div>
		</div>
		<div
			class="relative w-full h-[620px] bg-[var(--bg)] overflow-hidden"
			onWheel=${onWheel}
			onPointerDown=${onPointerDown}
			onPointerMove=${onPointerMove}
			onPointerUp=${stopDragging}
			onPointerLeave=${stopDragging}
			style="touch-action:none; cursor:grab;"
		>
			<svg viewBox=${`0 0 ${viewportWidth} ${viewportHeight}`} class="w-full h-full">
				<g transform=${`translate(${graphPanX.value} ${graphPanY.value}) scale(${graphZoom.value})`}>
					${graph.edges.map((edge) => {
						var from = layout.positions[edge.from];
						var to = layout.positions[edge.to];
						if (!from || !to) return null;
						var imagined = edge.kind === "imagined";
						var isSelectedEdge = edge.from === selectedId.value || edge.to === selectedId.value;
						return html`<line
							key=${edge.id}
							x1=${from.x + layout.nodeWidth / 2}
							y1=${from.y + layout.nodeHeight / 2}
							x2=${to.x + layout.nodeWidth / 2}
							y2=${to.y + layout.nodeHeight / 2}
							stroke=${imagined ? "rgba(245,158,11,0.9)" : isSelectedEdge ? "rgba(100,116,139,0.9)" : "rgba(113,113,122,0.46)"}
							stroke-width=${imagined ? "2.1" : isSelectedEdge ? "1.8" : "1.25"}
							stroke-dasharray=${imagined ? "6 4" : edge.kind === "contradicts" ? "4 4" : ""}
							opacity=${isSelectedEdge ? "1" : "0.72"}
						/>`;
					})}
					${graph.items.map((item) => {
						var pos = layout.positions[item.id];
						var style = nodeStyle(item);
						var active = selectedId.value === item.id;
						var weak = isWeakNode(item);
						return html`<g
							key=${item.id}
							transform=${`translate(${pos.x}, ${pos.y})`}
							onClick=${() => onNodeSelect(item)}
							style="cursor:pointer;"
						>
							${active
								? html`<rect
									x="-6"
									y="-6"
									width=${layout.nodeWidth + 12}
									height=${layout.nodeHeight + 12}
									rx="16"
									ry="16"
									fill=${style.accent}
									stroke="rgba(15,23,42,0.18)"
									stroke-width="1"
								/>`
								: null}
							<rect
								x="0"
								y="0"
								width=${layout.nodeWidth}
								height=${layout.nodeHeight}
								rx="12"
								ry="12"
								fill=${style.fill}
								stroke=${active ? "var(--text-strong)" : style.stroke}
								stroke-width=${active ? "2.8" : "1.4"}
								stroke-dasharray=${statusBucket(item) === "archived" ? "6 4" : ""}
								opacity=${weak ? "0.7" : "1"}
							/>
							<text x="12" y="18" font-size="10.5" fill="var(--muted)">
								${kindLabel(item)}${statusBucket(item) === "archived" ? " · archived" : ""}
							</text>
							<text x="12" y="35" font-size="12.5" fill=${style.text}>${compact(item.label, 24)}</text>
							<text x="12" y="48" font-size="10.5" fill="var(--muted)">
								${compact(summaryFor(item), 28) || item.id}
							</text>
						</g>`;
					})}
				</g>
			</svg>
		</div>
	</div>`;
}

function SelectionMeta({ item }) {
	if (!item) return html`<div class="text-sm text-[var(--muted)]">Select a node to inspect details.</div>`;
	if (item.kind === "imagined") {
		return html`<div class="space-y-3 text-sm">
			<div>
				<div class="text-[var(--text-strong)]">${item.title}</div>
				<div class="text-xs text-[var(--muted)]">hypothetical scenario · ${item.status}</div>
			</div>
			<div class="text-[var(--muted)]">${compact(item.premise, 180) || "No premise."}</div>
			<div class="text-xs text-[var(--muted)]">
				plausibility=${Number(item.plausibility_score || 0).toFixed(2)} novelty=${Number(item.novelty_score || 0).toFixed(2)}
				usefulness=${Number(item.usefulness_score || 0).toFixed(2)}
			</div>
			<div class="text-xs text-[var(--muted)]">basis: ${(item.basis_source_node_ids || []).slice(0, 4).join(", ") || "none"}</div>
		</div>`;
	}
	if (item.kind === "lesson") {
		return html`<div class="space-y-2 text-sm">
			<div class="text-[var(--text-strong)]">${item.title}</div>
			<div class="text-[var(--muted)]">${compact(item.statement, 180) || "No lesson statement."}</div>
			<div class="text-xs text-[var(--muted)]">
				confidence=${Number(item.confidence || 0).toFixed(2)} evidence=${item.evidence_count || 0}
			</div>
		</div>`;
	}
	if (item.kind === "checkpoint") {
		return html`<div class="space-y-2 text-sm">
			<div class="text-[var(--text-strong)]">${item.title}</div>
			<div class="text-[var(--muted)]">${compact(item.summary, 180) || "No checkpoint summary."}</div>
			<div class="text-xs text-[var(--muted)]">nodes=${(item.node_ids || []).length} lessons=${(item.lesson_ids || []).length}</div>
		</div>`;
	}
	if (item.kind === "trait") {
		return html`<div class="space-y-2 text-sm">
			<div class="text-[var(--text-strong)]">${item.label}</div>
			<div class="text-[var(--muted)]">${compact(item.description, 180) || "No trait description."}</div>
			<div class="text-xs text-[var(--muted)]">
				strength=${Number(item.strength || 0).toFixed(2)} confidence=${Number(item.confidence || 0).toFixed(2)}
			</div>
		</div>`;
	}
	if (detailLoading.value) return html`<div class="text-sm text-[var(--muted)]">Loading node detail...</div>`;
	if (!selectedDetail.value) {
		return html`<div class="text-sm text-[var(--muted)]">Node detail is unavailable for this item.</div>`;
	}
	return html`<div class="space-y-3 text-sm">
		<div>
			<div class="text-[var(--text-strong)]">${selectedDetail.value.node.title || item.title}</div>
			<div class="text-xs text-[var(--muted)]">
				type=${selectedDetail.value.node.node_type} · ${selectedDetail.value.node.status} · confidence=${Number(selectedDetail.value.node.confidence || 0).toFixed(2)}
				· importance=${Number(selectedDetail.value.node.importance || 0).toFixed(2)}
			</div>
		</div>
		<div class="text-[var(--muted)]">${compact(selectedDetail.value.summary, 220) || "No summary."}</div>
		${selectedDetail.value.content
			? html`<div class="rounded bg-[var(--surface2)] px-3 py-2 text-xs text-[var(--text)]">
				${compact(selectedDetail.value.content, 260)}
			</div>`
			: null}
		<div class="text-xs text-[var(--muted)]">source/provenance: ${selectedDetail.value.source_event_id || "none"}</div>
		<div>
			<div class="text-xs uppercase tracking-wide text-[var(--muted)] mb-1">Lesson links</div>
			${selectedDetail.value.lesson_links?.length
				? selectedDetail.value.lesson_links.slice(0, 4).map(
						(link) => html`<div class="text-xs text-[var(--text)]">${link.relation}: ${compact(link.title, 72)}</div>`,
					)
				: html`<div class="text-xs text-[var(--muted)]">No linked lessons.</div>`}
		</div>
		<div>
			<div class="text-xs uppercase tracking-wide text-[var(--muted)] mb-1">Related nodes</div>
			${selectedDetail.value.related_nodes?.length
				? selectedDetail.value.related_nodes.slice(0, 5).map(
						(node) => html`<div class="text-xs text-[var(--text)]">${node.relation}: ${compact(node.title, 58)} (${node.node_type})</div>`,
					)
				: html`<div class="text-xs text-[var(--muted)]">No related nodes.</div>`}
		</div>
	</div>`;
}

function selectedItem(data) {
	var items = buildItems(data);
	return items.find((item) => item.id === selectedId.value) || null;
}

function Sidebar() {
	var data = snapshot.value;
	var graph = visibleGraph(data);
	var item = selectedItem(data);
	var archivedCount = (data?.nodes || []).filter((node) => node.status === "archived").length;
	var imaginedCount = (data?.imagined_scenarios || []).length;
	return html`<div class="w-[340px] shrink-0 flex flex-col gap-3">
		<div class="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-4">
			<div class="flex items-center justify-between mb-3">
				<h3 class="text-sm font-medium text-[var(--text-strong)]">Graph Controls</h3>
				<button class="provider-btn provider-btn-secondary provider-btn-sm" onClick=${refreshSnapshot}>
					Refresh
				</button>
			</div>
			<div class="grid grid-cols-2 gap-2 text-xs mb-3">
				<div class="rounded bg-[var(--surface2)] px-2 py-2 text-[var(--muted)]">nodes ${data?.nodes?.length || 0}</div>
				<div class="rounded bg-[var(--surface2)] px-2 py-2 text-[var(--muted)]">edges ${data?.edges?.length || 0}</div>
				<div class="rounded bg-[var(--surface2)] px-2 py-2 text-[var(--muted)]">imagined ${imaginedCount}</div>
				<div class="rounded bg-[var(--surface2)] px-2 py-2 text-[var(--muted)]">archived ${archivedCount}</div>
			</div>
			<label class="block text-xs text-[var(--muted)] mb-1">Search</label>
			<input
				class="w-full mb-3 bg-[var(--bg)] border border-[var(--border)] rounded px-2 py-1.5 text-sm"
				type="text"
				placeholder="title, type, or node id"
				value=${searchQuery.value}
				onInput=${(e) => (searchQuery.value = e.target.value)}
			/>
			<label class="block text-xs text-[var(--muted)] mb-1">Node type</label>
			<select class="w-full mb-3 bg-[var(--bg)] border border-[var(--border)] rounded px-2 py-1.5 text-sm" value=${filterNodeType.value} onInput=${(e) => (filterNodeType.value = e.target.value)}>
				<option value="all">All verified nodes</option>
				${["episodic", "semantic", "entity", "goal", "preference", "prediction", "prediction_error"].map(
					(type) => html`<option value=${type}>${type}</option>`,
				)}
			</select>
			<label class="block text-xs text-[var(--muted)] mb-1">Status</label>
			<select class="w-full mb-3 bg-[var(--bg)] border border-[var(--border)] rounded px-2 py-1.5 text-sm" value=${filterStatus.value} onInput=${(e) => (filterStatus.value = e.target.value)}>
				<option value="all">All</option>
				<option value="active">Active</option>
				<option value="archived">Archived only</option>
				<option value="imagined">Imagined only</option>
			</select>
			<label class="flex items-center gap-2 text-sm text-[var(--text)] mb-2">
				<input type="checkbox" checked=${imaginedOnly.value} onInput=${(e) => (imaginedOnly.value = e.target.checked)} />
				Imagined only
			</label>
			<label class="flex items-center gap-2 text-sm text-[var(--text)] mb-2">
				<input type="checkbox" checked=${neighborhoodOnly.value} onInput=${(e) => (neighborhoodOnly.value = e.target.checked)} />
				Neighborhood of selected node
			</label>
			<label class="flex items-center gap-2 text-sm text-[var(--text)]">
				<input type="checkbox" checked=${hideWeakNodes.value} onInput=${(e) => (hideWeakNodes.value = e.target.checked)} />
				Hide weak low-confidence nodes
			</label>
			<div class="mt-3 text-xs text-[var(--muted)]">
				visible items ${graph.items.length} · visible edges ${graph.edges.length}
			</div>
		</div>

		<div class="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-4">
			<h3 class="text-sm font-medium text-[var(--text-strong)] mb-3">Selected Item</h3>
			<${SelectionMeta} item=${item} />
		</div>

		<div class="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-4">
			<h3 class="text-sm font-medium text-[var(--text-strong)] mb-2">Superseded Preference/Goal History</h3>
			<div class="space-y-2 max-h-[220px] overflow-y-auto">
				${(data?.superseded_history || []).length
					? data.superseded_history.map(
							(node) => html`<button
								key=${`history:${node.id}`}
								class="w-full text-left rounded border border-[var(--border)] bg-[var(--surface2)] px-3 py-2"
								onClick=${() => (selectedId.value = node.id)}
							>
								<div class="text-xs text-[var(--muted)]">${node.node_type} archived</div>
								<div class="text-sm text-[var(--text)]">${compact(titleFor(node), 42)}</div>
							</button>`,
						)
					: html`<div class="text-xs text-[var(--muted)]">No superseded history.</div>`}
			</div>
		</div>
	</div>`;
}

function MemoryGraphPage() {
	useEffect(() => {
		refreshSnapshot();
	}, []);

	var item = selectedItem(snapshot.value);
	useEffect(() => {
		if (!item || item.kind !== "node") {
			selectedDetail.value = null;
			return;
		}
		detailLoading.value = true;
		fetchJson(`/api/nodamem/graph/nodes/${encodeURIComponent(item.id)}`)
			.then((data) => {
				if (selectedId.value === item.id) selectedDetail.value = data;
			})
			.catch(() => {
				if (selectedId.value === item.id) selectedDetail.value = null;
			})
			.finally(() => {
				if (selectedId.value === item.id) detailLoading.value = false;
			});
	}, [selectedId.value, item?.kind]);

	return html`<div class="flex-1 min-w-0 p-4 overflow-y-auto">
		<div class="mb-4">
			<h2 class="text-base font-medium text-[var(--text-strong)]">Nodamem Graph</h2>
			<div class="text-sm text-[var(--muted)]">
				Internal graph viewer for verified memory, imagined planning scenarios, lessons, checkpoints, traits, and superseded history.
			</div>
		</div>
		${
			error.value
				? html`<div class="rounded-lg border border-[var(--error)] bg-[var(--error-bg)] px-4 py-3 text-sm text-[var(--error)] mb-4">
					${error.value}
				</div>`
				: null
		}
		<div class="flex gap-4 min-h-0 flex-col xl:flex-row">
			<div class="flex-1 min-w-0">${loading.value && !snapshot.value ? html`<div class="text-sm text-[var(--muted)]">Loading graph...</div>` : html`<${GraphCanvas} />`}</div>
			<${Sidebar} />
		</div>
	</div>`;
}

export function initMemoryGraph(container) {
	containerRef = container;
	container.style.cssText = "padding:0;overflow:hidden;";
	render(html`<${MemoryGraphPage} />`, container);
}

export function teardownMemoryGraph() {
	if (containerRef) render(null, containerRef);
	containerRef = null;
	snapshot.value = null;
	loading.value = false;
	error.value = "";
	selectedId.value = "";
	selectedDetail.value = null;
	detailLoading.value = false;
	filterNodeType.value = "all";
	filterStatus.value = "all";
	imaginedOnly.value = false;
	neighborhoodOnly.value = false;
	searchQuery.value = "";
	hideWeakNodes.value = false;
	graphZoom.value = 1;
	graphPanX.value = 0;
	graphPanY.value = 0;
}
