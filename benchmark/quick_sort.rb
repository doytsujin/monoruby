def quicksort(arr, first, last)
  if first < last
    p_index = partition(arr, first, last)
    quicksort(arr, first, p_index - 1)
    quicksort(arr, p_index + 1, last)
  end
  arr
end
  
def partition(arr, first, last)
  # first select one element from the list, can be any element. 
  # rearrange the list so all elements less than pivot are left of it, elements greater than pivot are right of it.
  pivot = arr[last]
  p_index = first
    
  i = first
  while i < last
    if arr[i] <= pivot
      arr[i], arr[p_index] = arr[p_index], arr[i]
      p_index += 1
    end
    i += 1
  end
  arr[p_index], arr[last] = pivot, arr[p_index]
  return p_index
end

puts quicksort([3,6,4,9,-1,58,-12,-19,1024,-1022,-510,768,200], 0, 12)