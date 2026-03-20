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
var selectedClusterId = signal("all");
var selectedNodePanel = signal("provenance");
var graphZoom = signal(1);
var graphPanX = signal(0);
var graphPanY = signal(0);
var hoveredPreview = signal(null);
var hoveredEdgeId = signal("");
var containerRef = null;

var CLUSTER_COLORS = [
	{ stroke: "#2563eb", fill: "rgba(37,99,235,0.08)", chip: "rgba(37,99,235,0.12)" },
	{ stroke: "#16a34a", fill: "rgba(22,163,74,0.08)", chip: "rgba(22,163,74,0.12)" },
	{ stroke: "#d97706", fill: "rgba(217,119,6,0.08)", chip: "rgba(217,119,6,0.12)" },
	{ stroke: "#7c3aed", fill: "rgba(124,58,237,0.08)", chip: "rgba(124,58,237,0.12)" },
	{ stroke: "#db2777", fill: "rgba(219,39,119,0.08)", chip: "rgba(219,39,119,0.12)" },
	{ stroke: "#0891b2", fill: "rgba(8,145,178,0.08)", chip: "rgba(8,145,178,0.12)" },
];

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

function clusterLabel(items) {
	var counts = new Map();
	items.forEach((item) => {
		var key = kindLabel(item);
		counts.set(key, (counts.get(key) || 0) + 1);
	});
	var dominant = Array.from(counts.entries()).sort((a, b) => b[1] - a[1])[0]?.[0] || "mixed";
	return `${dominant} cluster`;
}

function analyzeClusters(items, edges) {
	var byItemId = {};
	var adjacency = new Map(items.map((item) => [item.id, new Set()]));
	edges.forEach((edge) => {
		if (!adjacency.has(edge.from) || !adjacency.has(edge.to)) return;
		adjacency.get(edge.from).add(edge.to);
		adjacency.get(edge.to).add(edge.from);
	});

	var remaining = new Set(items.map((item) => item.id));
	var clusters = [];
	var index = 0;

	while (remaining.size > 0) {
		var start = remaining.values().next().value;
		var stack = [start];
		var ids = [];
		remaining.delete(start);
		while (stack.length) {
			var current = stack.pop();
			ids.push(current);
			(adjacency.get(current) || new Set()).forEach((next) => {
				if (!remaining.has(next)) return;
				remaining.delete(next);
				stack.push(next);
			});
		}

		var clusterItems = ids
			.map((id) => items.find((item) => item.id === id))
			.filter(Boolean);
		var color = CLUSTER_COLORS[index % CLUSTER_COLORS.length];
		var cluster = {
			id: `cluster:${index + 1}`,
			index,
			size: clusterItems.length,
			label: clusterLabel(clusterItems),
			itemIds: ids,
			color,
		};
		clusterItems.forEach((item) => {
			byItemId[item.id] = cluster.id;
		});
		clusters.push(cluster);
		index += 1;
	}

	clusters.sort((a, b) => b.size - a.size);
	clusters.forEach((cluster, order) => {
		cluster.index = order;
		cluster.color = CLUSTER_COLORS[order % CLUSTER_COLORS.length];
	});
	return { clusters, byItemId };
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

function visibleGraph(data) {
	var items = buildItems(data);
	var edges = buildEdges(data);
	var allowed = neighborhoodOnly.value ? buildNeighborhood(edges, selectedId.value) : null;
	var baseItems = items.filter((item) => isVisibleItem(item, allowed));
	var baseIds = new Set(baseItems.map((item) => item.id));
	var baseEdges = edges.filter((edge) => baseIds.has(edge.from) && baseIds.has(edge.to));
	var clusterInfo = analyzeClusters(baseItems, baseEdges);

	if (selectedClusterId.value !== "all") {
		var selectedCluster = clusterInfo.clusters.find((cluster) => cluster.id === selectedClusterId.value);
		if (selectedCluster) {
			var clusterIds = new Set(selectedCluster.itemIds);
			var itemsInCluster = baseItems.filter((item) => clusterIds.has(item.id));
			var edgesInCluster = baseEdges.filter((edge) => clusterIds.has(edge.from) && clusterIds.has(edge.to));
			return {
				items: itemsInCluster,
				edges: edgesInCluster,
				clusters: clusterInfo.clusters,
				allClusters: clusterInfo.clusters,
				clusterByItemId: clusterInfo.byItemId,
				activeClusterId: selectedCluster.id,
			};
		}
	}

	return {
		items: baseItems,
		edges: baseEdges,
		clusters: clusterInfo.clusters,
		allClusters: clusterInfo.clusters,
		clusterByItemId: clusterInfo.byItemId,
		activeClusterId: "all",
	};
}

function laneFor(item) {
	if (item.kind === "imagined") return 4;
	if (item.kind === "trait") return 3;
	if (item.kind === "lesson" || item.kind === "checkpoint") return 2;
	if (item.kind === "node" && (item.node_type === "goal" || item.node_type === "preference")) return 0;
	return 1;
}

function compareItems(a, b, clusterByItemId, clusterOrder) {
	var clusterA = clusterOrder.get(clusterByItemId[a.id]) ?? Number.MAX_SAFE_INTEGER;
	var clusterB = clusterOrder.get(clusterByItemId[b.id]) ?? Number.MAX_SAFE_INTEGER;
	if (clusterA !== clusterB) return clusterA - clusterB;
	if (statusBucket(a) !== statusBucket(b)) return statusBucket(a) === "active" ? -1 : 1;
	var aScore = scoreFor(a, "importance") ?? scoreFor(a, "confidence") ?? scoreFor(a, "strength") ?? 0;
	var bScore = scoreFor(b, "importance") ?? scoreFor(b, "confidence") ?? scoreFor(b, "strength") ?? 0;
	if (aScore !== bScore) return bScore - aScore;
	return titleFor(a).localeCompare(titleFor(b));
}

function layoutGraph(graph) {
	var laneWidth = 290;
	var laneStart = 90;
	var rowGap = 92;
	var clusterGap = 34;
	var nodeWidth = 190;
	var nodeHeight = 56;
	var lanes = [[], [], [], [], []];
	var clusterOrder = new Map(graph.clusters.map((cluster, index) => [cluster.id, index]));
	graph.items.forEach((item) => lanes[laneFor(item)].push(item));
	lanes.forEach((laneItems) => laneItems.sort((a, b) => compareItems(a, b, graph.clusterByItemId, clusterOrder)));

	var positions = {};
	var clusterBounds = {};

	lanes.forEach((laneItems, laneIndex) => {
		var rowIndex = 0;
		var previousCluster = "";
		laneItems.forEach((item) => {
			var clusterId = graph.clusterByItemId[item.id] || "";
			if (previousCluster && clusterId !== previousCluster) rowIndex += 1;
			var x = laneStart + laneIndex * laneWidth;
			var y = 70 + rowIndex * rowGap;
			positions[item.id] = { x, y };
			if (!clusterBounds[clusterId]) {
				clusterBounds[clusterId] = { minX: x, maxX: x + nodeWidth, minY: y, maxY: y + nodeHeight };
			} else {
				clusterBounds[clusterId].minX = Math.min(clusterBounds[clusterId].minX, x);
				clusterBounds[clusterId].maxX = Math.max(clusterBounds[clusterId].maxX, x + nodeWidth);
				clusterBounds[clusterId].minY = Math.min(clusterBounds[clusterId].minY, y);
				clusterBounds[clusterId].maxY = Math.max(clusterBounds[clusterId].maxY, y + nodeHeight);
			}
			previousCluster = clusterId;
			rowIndex += 1;
		});
	});

	var contentWidth = laneStart * 2 + laneWidth * lanes.length;
	var contentHeight = Math.max(460, ...Object.values(positions).map((p) => p.y + nodeHeight + 90), 460);
	return { positions, clusterBounds, nodeWidth, nodeHeight, width: contentWidth, height: contentHeight };
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

function edgeLabel(edge) {
	if (edge.kind === "supports") return "supports";
	if (edge.kind === "contradicts") return "contradicts";
	if (edge.kind === "checkpoint") return "checkpoint";
	if (edge.kind === "trait") return "trait support";
	if (edge.kind === "imagined") return "hypothesis basis";
	return edge.kind.replaceAll("_", " ");
}

function hoverStatus(item) {
	var status = [];
	if (item.kind === "imagined") status.push("imagined");
	if (statusBucket(item) === "archived") status.push("archived");
	if (item.status === "superseded") status.push("superseded");
	return status.join(" · ") || "active";
}

function setHoverPreview(item, event) {
	hoveredPreview.value = {
		item,
		x: event.clientX,
		y: event.clientY,
	};
}

function clearHoverPreview() {
	hoveredPreview.value = null;
}

function centerOnNode(nodeId, layout, viewportWidth, viewportHeight) {
	var pos = layout?.positions?.[nodeId];
	if (!pos) return;
	var scaledWidth = layout.nodeWidth * graphZoom.value;
	var scaledHeight = layout.nodeHeight * graphZoom.value;
	graphPanX.value = viewportWidth / 2 - (pos.x * graphZoom.value + scaledWidth / 2);
	graphPanY.value = viewportHeight / 2 - (pos.y * graphZoom.value + scaledHeight / 2);
}

function HoverPreview() {
	var hover = hoveredPreview.value;
	if (!hover?.item) return null;
	var item = hover.item;
	var confidence = scoreFor(item, "confidence");
	var importance = scoreFor(item, "importance");
	return html`<div
		class="absolute z-10 max-w-[280px] rounded-lg border border-[var(--border)] bg-[var(--surface)] shadow-lg px-3 py-2 pointer-events-none"
		style=${`left:${Math.min(hover.x + 16, window.innerWidth - 320)}px; top:${Math.max(16, hover.y + 16)}px;`}
	>
		<div class="text-sm text-[var(--text-strong)]">${compact(titleFor(item), 52)}</div>
		<div class="text-xs text-[var(--muted)]">${kindLabel(item)} · ${hoverStatus(item)}</div>
		<div class="mt-1 text-xs text-[var(--text)]">${compact(summaryFor(item), 120) || item.id}</div>
		${confidence !== null || importance !== null
			? html`<div class="mt-1 text-[11px] text-[var(--muted)]">
				${confidence !== null ? `confidence=${confidence.toFixed(2)}` : ""}
				${confidence !== null && importance !== null ? " · " : ""}
				${importance !== null ? `importance=${importance.toFixed(2)}` : ""}
			</div>`
			: null}
	</div>`;
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
		var clusterId = graph.clusterByItemId[item.id];
		if (clusterId && selectedClusterId.value !== "all" && selectedClusterId.value !== clusterId) {
			selectedClusterId.value = clusterId;
		}
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
			<div>Developer graph view. Cluster tinting groups related items. Amber is hypothetical. Muted or dashed nodes are archived or superseded.</div>
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
			onPointerLeave=${() => {
				stopDragging();
				hoveredEdgeId.value = "";
				clearHoverPreview();
			}}
			style="touch-action:none; cursor:grab;"
		>
			<svg viewBox=${`0 0 ${viewportWidth} ${viewportHeight}`} class="w-full h-full">
				<g transform=${`translate(${graphPanX.value} ${graphPanY.value}) scale(${graphZoom.value})`}>
					${graph.clusters.map((cluster) => {
						var bounds = layout.clusterBounds[cluster.id];
						if (!bounds) return null;
						return html`<g key=${`cluster-bg:${cluster.id}`}>
							<rect
								x=${bounds.minX - 28}
								y=${bounds.minY - 26}
								width=${bounds.maxX - bounds.minX + 56}
								height=${bounds.maxY - bounds.minY + 52}
								rx="24"
								ry="24"
								fill=${cluster.color.fill}
								stroke=${cluster.color.stroke}
								stroke-dasharray="4 6"
								stroke-width="1"
							/>
							<text x=${bounds.minX - 18} y=${bounds.minY - 8} font-size="11" fill=${cluster.color.stroke}>
								${cluster.label}
							</text>
						</g>`;
					})}
					${graph.edges.map((edge) => {
						var from = layout.positions[edge.from];
						var to = layout.positions[edge.to];
						if (!from || !to) return null;
						var imagined = edge.kind === "imagined";
						var isSelectedEdge = edge.from === selectedId.value || edge.to === selectedId.value;
						var showLabel = hoveredEdgeId.value === edge.id || isSelectedEdge;
						var midX = (from.x + to.x) / 2 + layout.nodeWidth / 2;
						var midY = (from.y + to.y) / 2 + layout.nodeHeight / 2;
						return html`<g key=${edge.id}>
							<line
								x1=${from.x + layout.nodeWidth / 2}
								y1=${from.y + layout.nodeHeight / 2}
								x2=${to.x + layout.nodeWidth / 2}
								y2=${to.y + layout.nodeHeight / 2}
								stroke=${imagined ? "rgba(245,158,11,0.9)" : isSelectedEdge ? "rgba(100,116,139,0.9)" : "rgba(113,113,122,0.46)"}
								stroke-width=${imagined ? "2.1" : isSelectedEdge ? "1.8" : "1.25"}
								stroke-dasharray=${imagined ? "6 4" : edge.kind === "contradicts" ? "4 4" : ""}
								opacity=${isSelectedEdge ? "1" : "0.72"}
								onMouseEnter=${() => (hoveredEdgeId.value = edge.id)}
								onMouseLeave=${() => (hoveredEdgeId.value = "")}
							/>
							${showLabel
								? html`<g>
									<rect
										x=${midX - 38}
										y=${midY - 10}
										width="76"
										height="18"
										rx="9"
										ry="9"
										fill="rgba(15,23,42,0.9)"
									/>
									<text x=${midX} y=${midY + 3} text-anchor="middle" font-size="10.5" fill="#f8fafc">
										${edgeLabel(edge)}
									</text>
								</g>`
								: null}
						</g>`;
					})}
					${graph.items.map((item) => {
						var pos = layout.positions[item.id];
						var style = nodeStyle(item);
						var active = selectedId.value === item.id;
						var weak = isWeakNode(item);
						var cluster = graph.clusters.find((entry) => entry.id === graph.clusterByItemId[item.id]);
						return html`<g
							key=${item.id}
							transform=${`translate(${pos.x}, ${pos.y})`}
							onClick=${() => onNodeSelect(item)}
							onMouseEnter=${(event) => setHoverPreview(item, event)}
							onMouseLeave=${clearHoverPreview}
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
							${cluster
								? html`<rect x="0" y="0" width="6" height=${layout.nodeHeight} rx="12" ry="12" fill=${cluster.color.stroke} />`
								: null}
							<text x="12" y="18" font-size="10.5" fill="var(--muted)">
								${kindLabel(item)}${statusBucket(item) === "archived" ? " · archived" : ""}
							</text>
							<text x="12" y="35" font-size="12.5" fill=${style.text}>${compact(item.label, 24)}</text>
							<text x="12" y="49" font-size="10.5" fill="var(--muted)">
								${compact(summaryFor(item), 28) || item.id}
							</text>
						</g>`;
					})}
				</g>
			</svg>
			<${HoverPreview} />
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

function normalizedText(value) {
	return String(value || "")
		.toLowerCase()
		.replace(/[^a-z0-9\s]/g, " ")
		.trim();
}

function keywordSet(item) {
	return new Set(
		normalizedText(`${titleFor(item)} ${summaryFor(item)}`)
			.split(/\s+/)
			.filter((part) => part.length >= 4),
	);
}

function sameTopicHistory(currentItem, historyItems) {
	if (!currentItem || currentItem.kind !== "node") return [];
	var currentWords = keywordSet(currentItem);
	return (historyItems || []).filter((candidate) => {
		if (candidate.id === currentItem.id) return false;
		if (candidate.node_type !== currentItem.node_type) return false;
		var candidateWords = keywordSet(candidate);
		var overlap = 0;
		currentWords.forEach((word) => {
			if (candidateWords.has(word)) overlap += 1;
		});
		return overlap > 0;
	});
}

function supportingTraitsForNode(data, detail) {
	if (!detail?.node?.id) return [];
	var lessonIds = new Set((detail.lesson_links || []).map((link) => link.lesson_id));
	return (data?.traits || []).filter((trait) => {
		var nodeMatch = (trait.supporting_node_ids || []).includes(detail.node.id);
		var lessonMatch = (trait.supporting_lesson_ids || []).some((lessonId) => lessonIds.has(String(lessonId)));
		return nodeMatch || lessonMatch;
	});
}

function nodeStateSummary(detail) {
	var reasons = detail?.reasons || [];
	var states = [];
	if (reasons.some((reason) => /reinforc/i.test(reason))) states.push("reinforced");
	if (reasons.some((reason) => /refin/i.test(reason))) states.push("refined");
	if (reasons.some((reason) => /weaken|contradict/i.test(reason))) states.push("weakened");
	if (reasons.some((reason) => /supersed/i.test(reason))) states.push("superseded");
	return states.length ? states.join(" · ") : "no recent state change noted";
}

function neighboringClusterNodes(graph, detail) {
	if (!detail?.node?.id) return [];
	var selectedCluster = graph.clusterByItemId[detail.node.id];
	if (!selectedCluster) return [];
	var relatedIds = new Set((detail.related_nodes || []).map((node) => node.node_id));
	return graph.items.filter((item) => {
		if (item.id === detail.node.id) return false;
		if (!relatedIds.has(item.id)) return false;
		return graph.clusterByItemId[item.id] === selectedCluster;
	});
}

function NodeActionPanel({ item, graph, data }) {
	if (!item) return html`<div class="text-sm text-[var(--muted)]">Select a node to inspect actions.</div>`;
	if (item.kind === "imagined") {
		return html`<div class="space-y-2 text-sm">
			<div class="text-[var(--text-strong)]">Hypothetical scenario</div>
			<div class="text-[var(--muted)]">
				Imagined content stays separate from verified provenance. Review its basis nodes in the graph and use the main details card for scenario context.
			</div>
			<div class="text-xs text-[var(--muted)]">basis nodes: ${(item.basis_source_node_ids || []).slice(0, 5).join(", ") || "none"}</div>
		</div>`;
	}
	if (item.kind !== "node") {
		return html`<div class="text-sm text-[var(--muted)]">Node actions are available for verified memory nodes.</div>`;
	}
	if (detailLoading.value) return html`<div class="text-sm text-[var(--muted)]">Loading node actions...</div>`;
	if (!selectedDetail.value) return html`<div class="text-sm text-[var(--muted)]">Node detail is unavailable for actions.</div>`;

	var detail = selectedDetail.value;
	var traits = supportingTraitsForNode(data, detail);
	var sameClusterNodes = neighboringClusterNodes(graph, detail);
	var superseded = sameTopicHistory(item, data?.superseded_history || []);

	if (selectedNodePanel.value === "lessons") {
		return html`<div class="space-y-2 text-sm">
			<div class="text-xs uppercase tracking-wide text-[var(--muted)]">Related lesson history</div>
			${detail.lesson_links?.length
				? detail.lesson_links.map(
						(link) => html`<div class="rounded bg-[var(--surface2)] px-3 py-2 text-xs text-[var(--text)]">
							<div class="text-[var(--muted)]">${link.relation}</div>
							<div>${compact(link.title, 80)}</div>
						</div>`,
					)
				: html`<div class="text-xs text-[var(--muted)]">No supporting or contradicting lessons linked to this node.</div>`}
		</div>`;
	}

	if (selectedNodePanel.value === "history") {
		return html`<div class="space-y-2 text-sm">
			<div class="text-xs uppercase tracking-wide text-[var(--muted)]">Supersession history</div>
			<div class="text-xs text-[var(--muted)]">${nodeStateSummary(detail)}</div>
			${superseded.length
				? superseded.map(
						(node) => html`<button
							key=${`superseded:${node.id}`}
							class="w-full text-left rounded border border-[var(--border)] bg-[var(--surface2)] px-3 py-2"
							onClick=${() => (selectedId.value = node.id)}
						>
							<div class="text-xs text-[var(--muted)]">${node.node_type} · archived</div>
							<div class="text-sm text-[var(--text)]">${compact(titleFor(node), 72)}</div>
						</button>`,
					)
				: html`<div class="text-xs text-[var(--muted)]">No same-topic superseded history was found in the current snapshot.</div>`}
		</div>`;
	}

	if (selectedNodePanel.value === "neighbors") {
		return html`<div class="space-y-2 text-sm">
			<div class="text-xs uppercase tracking-wide text-[var(--muted)]">Neighbor jump</div>
			<div class="text-xs text-[var(--muted)]">Jump to nearby nodes that remain in the same current cluster/topic.</div>
			${sameClusterNodes.length
				? sameClusterNodes.slice(0, 8).map(
						(node) => html`<button
							key=${`neighbor:${node.id}`}
							class="w-full text-left rounded border border-[var(--border)] bg-[var(--surface2)] px-3 py-2"
							onClick=${() => {
								selectedClusterId.value = graph.clusterByItemId[node.id] || "all";
								selectedId.value = node.id;
							}}
						>
							<div class="text-xs text-[var(--muted)]">${kindLabel(node)}</div>
							<div class="text-sm text-[var(--text)]">${compact(titleFor(node), 72)}</div>
						</button>`,
					)
				: html`<div class="text-xs text-[var(--muted)]">No same-cluster neighbor candidates are available for this node.</div>`}
		</div>`;
	}

	return html`<div class="space-y-3 text-sm">
		<div class="text-xs uppercase tracking-wide text-[var(--muted)]">Provenance and evidence</div>
		<div class="text-xs text-[var(--muted)]">state: ${nodeStateSummary(detail)}</div>
		<div>
			<div class="text-xs uppercase tracking-wide text-[var(--muted)] mb-1">Source nodes</div>
			${detail.related_nodes?.length
				? detail.related_nodes.slice(0, 5).map(
						(node) => html`<button
							key=${`source:${node.node_id}`}
							class="w-full text-left rounded bg-[var(--surface2)] px-3 py-2 text-xs text-[var(--text)] mb-1"
							onClick=${() => (selectedId.value = node.node_id)}
						>
							${compact(node.title, 72)} (${node.node_type})
						</button>`,
					)
				: html`<div class="text-xs text-[var(--muted)]">No neighboring source nodes recorded.</div>`}
		</div>
		<div>
			<div class="text-xs uppercase tracking-wide text-[var(--muted)] mb-1">Supporting lessons</div>
			${detail.lesson_links?.length
				? detail.lesson_links.slice(0, 4).map(
						(link) => html`<div class="rounded bg-[var(--surface2)] px-3 py-2 text-xs text-[var(--text)] mb-1">
							${link.relation}: ${compact(link.title, 74)}
						</div>`,
					)
				: html`<div class="text-xs text-[var(--muted)]">No supporting lessons linked.</div>`}
		</div>
		<div>
			<div class="text-xs uppercase tracking-wide text-[var(--muted)] mb-1">Trait or self-model influence</div>
			${traits.length
				? traits.slice(0, 3).map(
						(trait) => html`<div class="rounded bg-[var(--surface2)] px-3 py-2 text-xs text-[var(--text)] mb-1">
							${trait.label} (${Number(trait.strength || 0).toFixed(2)})
						</div>`,
					)
				: detail.reasons.some((reason) => /self-model/i.test(reason))
					? html`<div class="text-xs text-[var(--text)]">Recent self-model influence was mentioned in audit reasons.</div>`
					: html`<div class="text-xs text-[var(--muted)]">No direct trait or self-model influence is linked in the current inspection payload.</div>`}
		</div>
		<div class="text-xs text-[var(--muted)]">source event: ${detail.source_event_id || "none"}</div>
	</div>`;
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
				onInput=${(e) => {
					searchQuery.value = e.target.value;
					selectedClusterId.value = "all";
				}}
			/>
			<label class="block text-xs text-[var(--muted)] mb-1">Cluster/topic</label>
			<div class="flex gap-2 mb-3">
				<select
					class="flex-1 bg-[var(--bg)] border border-[var(--border)] rounded px-2 py-1.5 text-sm"
					value=${selectedClusterId.value}
					onInput=${(e) => (selectedClusterId.value = e.target.value)}
				>
					<option value="all">Full graph</option>
					${graph.allClusters.map(
						(cluster) => html`<option key=${cluster.id} value=${cluster.id}>
							${cluster.label} (${cluster.size})
						</option>`,
					)}
				</select>
				<button class="provider-btn provider-btn-secondary provider-btn-sm" onClick=${() => (selectedClusterId.value = "all")}>
					Full graph
				</button>
			</div>
			<div class="flex flex-wrap gap-1.5 mb-3">
				${graph.allClusters.slice(0, 8).map(
					(cluster) => html`<button
						key=${`chip:${cluster.id}`}
						class="rounded-full border px-2 py-1 text-[11px]"
						style=${`border-color:${cluster.color.stroke}; background:${selectedClusterId.value === cluster.id ? cluster.color.chip : "transparent"}; color:var(--text);`}
						onClick=${() => (selectedClusterId.value = selectedClusterId.value === cluster.id ? "all" : cluster.id)}
					>
						${cluster.label}
					</button>`,
				)}
			</div>
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
				clusters ${graph.allClusters.length} · visible items ${graph.items.length} · visible edges ${graph.edges.length}
			</div>
		</div>

		<div class="rounded-lg border border-[var(--border)] bg-[var(--surface)] p-4">
			<div class="flex items-center justify-between mb-3">
				<h3 class="text-sm font-medium text-[var(--text-strong)]">Selected Item</h3>
				${item?.kind === "node"
					? html`<button class="provider-btn provider-btn-secondary provider-btn-sm" disabled title="No archive action is exposed by the current backend inspection path">
						Archive unavailable
					</button>`
					: null}
			</div>
			<${SelectionMeta} item=${item} />
			${item?.kind === "node"
				? html`<div class="mt-4">
					<div class="text-xs uppercase tracking-wide text-[var(--muted)] mb-2">Node actions</div>
					<div class="flex flex-wrap gap-1.5 mb-3">
						${[
							["provenance", "Provenance"],
							["lessons", "Lessons"],
							["history", "History"],
							["neighbors", "Neighbors"],
						].map(
							([value, label]) => html`<button
								key=${`panel:${value}`}
								class="rounded-full border px-2 py-1 text-[11px]"
								style=${`border-color:var(--border); background:${selectedNodePanel.value === value ? "var(--surface2)" : "transparent"}; color:var(--text);`}
								onClick=${() => (selectedNodePanel.value = value)}
							>
								${label}
							</button>`,
						)}
					</div>
					<${NodeActionPanel} item=${item} graph=${graph} data=${data} />
				</div>`
				: null}
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

	var graph = visibleGraph(snapshot.value);
	useEffect(() => {
		if (selectedClusterId.value !== "all" && !graph.allClusters.some((cluster) => cluster.id === selectedClusterId.value)) {
			selectedClusterId.value = "all";
		}
	}, [graph.allClusters.length, selectedClusterId.value]);

	var item = selectedItem(snapshot.value);
	useEffect(() => {
		selectedNodePanel.value = "provenance";
	}, [selectedId.value]);

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
	selectedClusterId.value = "all";
	selectedNodePanel.value = "provenance";
	graphZoom.value = 1;
	graphPanX.value = 0;
	graphPanY.value = 0;
	hoveredPreview.value = null;
	hoveredEdgeId.value = "";
}
