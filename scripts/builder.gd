extends Node3D

@export var structures: Array[Structure] = []

var map: DataMap

var index: int = 0 # Index of structure being built

@export var selector: Node3D # The 'cursor'
@export var selector_container: Node3D # Node that holds a preview of the structure
@export var view_camera: Camera3D # Used for raycasting mouse
@export var gridmap: GridMap
@export var cash_display: Label
@export var wallgrid: WallGrid

@onready var wall_mesh = ResourceLoader.load("models/wall.tscn")
@onready var door_mesh = ResourceLoader.load("models/door.tscn")

var cur_y: int = 0 # The current layer to interact with
var plane: Plane # Used for raycasting mouse
var drag_start: Vector3

func _ready():
	map = DataMap.new()
	plane = Plane(Vector3.UP, Vector3.ZERO)
	
	# Create new MeshLibrary dynamically, can also be done in the editor
	# See: https://docs.godotengine.org/en/stable/tutorials/3d/using_gridmaps.html
	
	var mesh_library = MeshLibrary.new()
	
	for structure in structures:
		var id = mesh_library.get_last_unused_item_id()
		
		mesh_library.create_item(id)
		mesh_library.set_item_mesh(id, get_mesh(structure.model))
		mesh_library.set_item_mesh_transform(id, Transform3D())
		
	gridmap.mesh_library = mesh_library
	
	# WallGrid MeshLibrary
	var wallgrid_mesh_library = MeshLibrary.new()

	var wall_id = wallgrid_mesh_library.get_last_unused_item_id()
	wallgrid_mesh_library.create_item(wall_id)
	wallgrid_mesh_library.set_item_mesh(wall_id, get_mesh(wall_mesh))

	var door_id = wallgrid_mesh_library.get_last_unused_item_id()
	wallgrid_mesh_library.create_item(door_id)
	wallgrid_mesh_library.set_item_mesh(door_id, get_mesh(door_mesh))

	wallgrid.set_mesh_library(wallgrid_mesh_library)
	
	update_structure()
	update_cash()

	load_map()

func _process(_delta):
	# Controls
	accept_actions()
	
	# Map position based on mouse
	
	var world_position = plane.intersects_ray(
		view_camera.project_ray_origin(get_viewport().get_mouse_position()),
		view_camera.project_ray_normal(get_viewport().get_mouse_position()))

	if Input.is_action_just_pressed("build"):
		drag_start = world_position

	if Input.is_action_just_released("build"):
		self.wallgrid.paint_wall(drag_start, world_position)

	if Input.is_action_just_pressed("y_layer_up"):
		cur_y += 1
		if cur_y > 10:
			cur_y = 10
		plane.d = cur_y

	if Input.is_action_just_pressed("y_layer_down"):
		cur_y -= 1
		if cur_y < 0:
			cur_y = 0
		plane.d = cur_y


# Retrieve the mesh from a PackedScene, used for dynamically creating a MeshLibrary

func get_mesh(packed_scene):
	var scene_state: SceneState = packed_scene.get_state()
	for i in range(scene_state.get_node_count()):
		if (scene_state.get_node_type(i) == "MeshInstance3D"):
			for j in scene_state.get_node_property_count(i):
				var prop_name = scene_state.get_node_property_name(i, j)
				if prop_name == "mesh":
					var prop_value = scene_state.get_node_property_value(i, j)
					
					return prop_value.duplicate()

# Update the structure visual in the 'cursor' (obsolete)

func update_structure():
	# Clear previous structure preview in selector
	for n in selector_container.get_children():
		selector_container.remove_child(n)
		
	# Create new structure preview in selector
	var _model = structures[index].model.instantiate()
	selector_container.add_child(_model)
	_model.position.y += 0.25
	
func update_cash():
	cash_display.text = "$" + str(map.cash)

# Saving/load

func load_map():
	print("Loading map...")
	
	gridmap.clear()
	
	map = ResourceLoader.load("user://map.res")
	if not map:
		map = DataMap.new()
	for cell in map.structures:
		gridmap.set_cell_item(Vector3i(cell.position.x, cur_y, cell.position.y), cell.structure, cell.orientation)
		
	update_cash()

func accept_actions():
	if Input.is_action_just_pressed("save"):
		print("Saving map...")
		
		map.structures.clear()
		for cell in gridmap.get_used_cells():
			var data_structure: DataStructure = DataStructure.new()
			
			data_structure.position = Vector2i(cell.x, cell.z)
			data_structure.orientation = gridmap.get_cell_item_orientation(cell)
			data_structure.structure = gridmap.get_cell_item(cell)
			
			map.structures.append(data_structure)
			
		ResourceSaver.save(map, "user://map.res")

	if Input.is_action_just_pressed("load"):
		load_map()

	if Input.is_action_just_pressed("quit"):
		get_tree().quit()
