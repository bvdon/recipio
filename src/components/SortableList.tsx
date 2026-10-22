import type { ReactNode } from "react";
import {
  closestCenter,
  DndContext,
  type DragEndEvent,
  KeyboardSensor,
  PointerSensor,
  useSensor,
  useSensors,
} from "@dnd-kit/core";
import {
  arrayMove,
  SortableContext,
  sortableKeyboardCoordinates,
  useSortable,
  verticalListSortingStrategy,
} from "@dnd-kit/sortable";
import { CSS } from "@dnd-kit/utilities";
import { GripVertical } from "lucide-react";

type Props<T extends { key: string }> = {
  items: T[];
  onReorder: (items: T[]) => void;
  renderItem: (item: T, index: number, handle: ReactNode) => ReactNode;
  className?: string;
};

/** Vertical list reorderable by mouse or keyboard (focus the handle, Space, arrows, Space). */
export default function SortableList<T extends { key: string }>({
  items,
  onReorder,
  renderItem,
  className,
}: Props<T>) {
  const sensors = useSensors(
    useSensor(PointerSensor, { activationConstraint: { distance: 4 } }),
    useSensor(KeyboardSensor, { coordinateGetter: sortableKeyboardCoordinates }),
  );

  const onDragEnd = ({ active, over }: DragEndEvent) => {
    if (!over || active.id === over.id) return;
    const from = items.findIndex((i) => i.key === active.id);
    const to = items.findIndex((i) => i.key === over.id);
    onReorder(arrayMove(items, from, to));
  };

  return (
    <DndContext sensors={sensors} collisionDetection={closestCenter} onDragEnd={onDragEnd}>
      <SortableContext items={items.map((i) => i.key)} strategy={verticalListSortingStrategy}>
        <ul className={className}>
          {items.map((item, index) => (
            <SortableRow key={item.key} id={item.key}>
              {(handle) => renderItem(item, index, handle)}
            </SortableRow>
          ))}
        </ul>
      </SortableContext>
    </DndContext>
  );
}

function SortableRow({ id, children }: { id: string; children: (handle: ReactNode) => ReactNode }) {
  const {
    attributes,
    listeners,
    setNodeRef,
    setActivatorNodeRef,
    transform,
    transition,
    isDragging,
  } = useSortable({ id });
  const handle = (
    <button
      type="button"
      className="drag-handle"
      ref={setActivatorNodeRef}
      {...attributes}
      {...listeners}
      aria-label="Drag to reorder"
      title="Drag to reorder"
    >
      <GripVertical size={16} aria-hidden />
    </button>
  );
  return (
    <li
      ref={setNodeRef}
      className={isDragging ? "sortable-row dragging" : "sortable-row"}
      style={{ transform: CSS.Transform.toString(transform), transition }}
    >
      {children(handle)}
    </li>
  );
}
